use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use super::resample::{self, Resampler};
use super::TARGET_SAMPLE_RATE;

use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// Microphone capture using WASAPI on Windows.
/// Captures from the default input device and converts to PCM s16le 16kHz mono.
/// macOS/Linux/Android use `cpal_mic.rs` instead.
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

    /// Start capturing from the microphone.
    /// Returns a receiver that yields PCM s16le 16kHz mono audio chunks.
    pub fn start(&mut self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        if self.is_capturing.load(Ordering::SeqCst) {
            return Err("Already capturing".to_string());
        }

        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        self.is_capturing.store(true, Ordering::SeqCst);
        let is_capturing = self.is_capturing.clone();

        // Handshake channel so we can return errors from the worker thread.
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let worker = std::thread::spawn(move || {
            let result: Result<(), String> = (|| {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

                    tracing::info!("[Mic] Starting WASAPI microphone capture...");

                    let enumerator: IMMDeviceEnumerator =
                        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                            .map_err(|e| format!("Failed to create device enumerator: {}", e))?;

                    let device = enumerator
                        .GetDefaultAudioEndpoint(eCapture, eConsole)
                        .map_err(|e| format!("Failed to get default capture endpoint: {}", e))?;

                    let audio_client: IAudioClient = device
                        .Activate(CLSCTX_ALL, None)
                        .map_err(|e| format!("Failed to activate audio client: {}", e))?;

                    let mix_format_ptr = audio_client
                        .GetMixFormat()
                        .map_err(|e| format!("GetMixFormat failed: {}", e))?;
                    let mix_format = &*mix_format_ptr;

                    let source_rate = mix_format.nSamplesPerSec;
                    let source_channels = mix_format.nChannels as u32;
                    let bits_per_sample = mix_format.wBitsPerSample;

                    tracing::info!(
                        "[Mic] Mix format: rate={}, channels={}, bits={}",
                        source_rate,
                        source_channels,
                        bits_per_sample
                    );

                    tracing::info!("[Mic] Initializing audio client...");
                    audio_client
                        .Initialize(
                            AUDCLNT_SHAREMODE_SHARED,
                            0,
                            10_000_000, // 1 second buffer in 100ns units
                            0,
                            mix_format_ptr,
                            None,
                        )
                        .map_err(|e| format!("AudioClient Initialize failed: {}", e))?;
                    tracing::info!("[Mic] Audio client initialized.");

                    let capture_client: IAudioCaptureClient = audio_client
                        .GetService()
                        .map_err(|e| format!("Failed to get capture client: {}", e))?;

                    tracing::info!("[Mic] Starting audio client...");
                    audio_client
                        .Start()
                        .map_err(|e| format!("Failed to start audio client: {}", e))?;
                    tracing::info!("[Mic] Audio client started.");

                    // Signal readiness only after capture starts.
                    let _ = ready_tx.send(Ok(()));

                    let mut resampler = Resampler::new(source_rate, TARGET_SAMPLE_RATE);

                    while is_capturing.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(10));

                        let packet_size = match capture_client.GetNextPacketSize() {
                            Ok(size) => size,
                            Err(_) => continue,
                        };
                        if packet_size == 0 {
                            continue;
                        }

                        let mut buffer_ptr = std::ptr::null_mut();
                        let mut num_frames = 0u32;
                        let mut flags = 0u32;

                        if capture_client
                            .GetBuffer(&mut buffer_ptr, &mut num_frames, &mut flags, None, None)
                            .is_err()
                        {
                            continue;
                        }

                        if num_frames > 0 && !buffer_ptr.is_null() {
                            let is_silent = (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0;
                            if !is_silent {
                                let pcm = convert_mic_buffer_to_pcm_s16_16k(
                                    buffer_ptr,
                                    num_frames,
                                    source_channels,
                                    bits_per_sample,
                                    &mut resampler,
                                );
                                if !pcm.is_empty() {
                                    let _ = sender.send(pcm);
                                }
                            }
                        }

                        let _ = capture_client.ReleaseBuffer(num_frames);
                    }

                    let _ = audio_client.Stop();
                    CoUninitialize();

                    Ok(())
                }
            })();

            // If we failed before signalling readiness, propagate the error once.
            if let Err(e) = result {
                let _ = ready_tx.send(Err(e));
            }
        });

        self.worker = Some(worker);

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                self.worker.take().map(|h| h.join());
                return Err(e);
            }
            Err(e) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                self.worker.take().map(|h| h.join());
                return Err(format!("Microphone worker failed to start: {}", e));
            }
        }

        Ok(receiver)
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

/// Convert a raw WASAPI mic buffer to PCM s16le 16kHz mono.
/// Handles 16-bit int and 32-bit float sources, mixes all channels down to
/// mono (average), then runs the shared filtered resampler.
unsafe fn convert_mic_buffer_to_pcm_s16_16k(
    buffer_ptr: *mut u8,
    num_frames: u32,
    source_channels: u32,
    bits_per_sample: u16,
    resampler: &mut Resampler,
) -> Vec<u8> {
    let frame_count = num_frames as usize;
    let channels = source_channels.max(1) as usize;

    let mono_f32: Vec<f32> = match bits_per_sample {
        16 => {
            let ptr = buffer_ptr as *const i16;
            let samples = std::slice::from_raw_parts(ptr, frame_count * channels);
            resample::mixdown_i16(samples, channels)
        }
        32 => {
            let ptr = buffer_ptr as *const f32;
            let samples = std::slice::from_raw_parts(ptr, frame_count * channels);
            resample::mixdown_f32(samples, channels)
        }
        _ => return Vec::new(),
    };

    resample::f32_to_s16le(&resampler.process(&mono_f32))
}
