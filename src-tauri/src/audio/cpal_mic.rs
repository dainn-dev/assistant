use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::resample::{self, Resampler};
use super::TARGET_SAMPLE_RATE;

/// Microphone capture via cpal — CoreAudio on macOS, ALSA/PulseAudio on Linux,
/// AAudio on Android. Outputs PCM s16le, 16 kHz, mono.
///
/// Windows uses the bespoke WASAPI implementation in `microphone.rs` instead.
///
/// macOS note: the app bundle must declare `NSMicrophoneUsageDescription`
/// (see `src-tauri/Info.plist`) or the OS will deny the input stream.
pub struct MicCapture {
    is_capturing: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl MicCapture {
    pub fn new() -> Self {
        Self {
            is_capturing: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }

    pub fn start(&mut self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        if self.is_capturing.load(Ordering::SeqCst) {
            return Err("Already capturing".to_string());
        }

        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        self.is_capturing.store(true, Ordering::SeqCst);
        let is_capturing = self.is_capturing.clone();

        // Handshake channel so stream-build errors propagate to start().
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let worker = std::thread::spawn(move || {
            let result: Result<(), String> = (|| {
                let host = cpal::default_host();
                let device = host
                    .default_input_device()
                    .ok_or("No default input device (microphone)")?;

                let default_config = device
                    .default_input_config()
                    .map_err(|e| format!("default input config: {e}"))?;

                let sample_format = default_config.sample_format();
                let cfg: cpal::StreamConfig = default_config.into();

                let channels = cfg.channels as usize;
                let source_rate = cfg.sample_rate.0;

                let shared_buf: Arc<Mutex<Vec<u8>>> =
                    Arc::new(Mutex::new(Vec::with_capacity(32000)));
                // Filtered resampler shared by the callback; one per stream.
                let resampler: Arc<Mutex<Resampler>> =
                    Arc::new(Mutex::new(Resampler::new(source_rate, TARGET_SAMPLE_RATE)));

                let err_fn = move |err| {
                    tracing::error!("[Mic] stream error: {err}");
                };

                // Keep chunks reasonably sized for IPC (100ms at 16kHz * 2 bytes)
                const FLUSH_BYTES: usize = 3200;

                // Convert a callback's mono f32 samples into the shared byte buffer,
                // flushing to the channel once a batch is large enough.
                let push_pcm =
                    |pcm: &[u8], buf: &Arc<Mutex<Vec<u8>>>, tx: &mpsc::Sender<Vec<u8>>| {
                        if pcm.is_empty() {
                            return;
                        }
                        if let Ok(mut b) = buf.lock() {
                            b.extend_from_slice(pcm);
                            if b.len() >= FLUSH_BYTES {
                                let out = std::mem::take(&mut *b);
                                let _ = tx.send(out);
                            }
                        }
                    };

                let stream = match sample_format {
                    cpal::SampleFormat::I16 => {
                        let buf = shared_buf.clone();
                        let res = resampler.clone();
                        let tx = sender.clone();
                        let flag = is_capturing.clone();
                        device
                            .build_input_stream(
                                &cfg,
                                move |data: &[i16], _| {
                                    if !flag.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    let mono = resample::mixdown_i16(data, channels);
                                    let pcm = match res.lock() {
                                        Ok(mut r) => resample::f32_to_s16le(&r.process(&mono)),
                                        Err(_) => return,
                                    };
                                    push_pcm(&pcm, &buf, &tx);
                                },
                                err_fn,
                                None,
                            )
                            .map_err(|e| format!("build_input_stream(i16): {e}"))?
                    }
                    cpal::SampleFormat::F32 => {
                        let buf = shared_buf.clone();
                        let res = resampler.clone();
                        let tx = sender.clone();
                        let flag = is_capturing.clone();
                        device
                            .build_input_stream(
                                &cfg,
                                move |data: &[f32], _| {
                                    if !flag.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    let mono = resample::mixdown_f32(data, channels);
                                    let pcm = match res.lock() {
                                        Ok(mut r) => resample::f32_to_s16le(&r.process(&mono)),
                                        Err(_) => return,
                                    };
                                    push_pcm(&pcm, &buf, &tx);
                                },
                                err_fn,
                                None,
                            )
                            .map_err(|e| format!("build_input_stream(f32): {e}"))?
                    }
                    cpal::SampleFormat::U16 => {
                        let buf = shared_buf.clone();
                        let res = resampler.clone();
                        let tx = sender.clone();
                        let flag = is_capturing.clone();
                        device
                            .build_input_stream(
                                &cfg,
                                move |data: &[u16], _| {
                                    if !flag.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    let as_f32: Vec<f32> = data
                                        .iter()
                                        .map(|&x| (x as f32 - 32768.0) / 32768.0)
                                        .collect();
                                    let mono = resample::mixdown_f32(&as_f32, channels);
                                    let pcm = match res.lock() {
                                        Ok(mut r) => resample::f32_to_s16le(&r.process(&mono)),
                                        Err(_) => return,
                                    };
                                    push_pcm(&pcm, &buf, &tx);
                                },
                                err_fn,
                                None,
                            )
                            .map_err(|e| format!("build_input_stream(u16): {e}"))?
                    }
                    other => {
                        return Err(format!("Unsupported mic sample format: {other:?}"));
                    }
                };

                stream.play().map_err(|e| format!("stream.play: {e}"))?;
                let _ = ready_tx.send(Ok(()));

                // Run until stopped.
                while is_capturing.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }

                // Flush remaining buffer.
                if let Ok(mut buf) = shared_buf.lock() {
                    if !buf.is_empty() {
                        let out = std::mem::take(&mut *buf);
                        let _ = sender.send(out);
                    }
                }

                drop(stream);
                Ok(())
            })();

            if let Err(e) = result {
                let _ = ready_tx.send(Err(e));
            }
        });

        self.worker = Some(worker);

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(receiver),
            Ok(Err(e)) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                self.worker.take().map(|h| h.join());
                Err(e)
            }
            Err(e) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                self.worker.take().map(|h| h.join());
                Err(format!("Microphone worker failed to start: {e}"))
            }
        }
    }

    pub fn stop(&mut self) {
        self.is_capturing.store(false, Ordering::SeqCst);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }

    #[allow(dead_code)]
    pub fn is_capturing(&self) -> bool {
        self.is_capturing.load(Ordering::SeqCst)
    }
}

impl Default for MicCapture {
    fn default() -> Self {
        Self::new()
    }
}
