use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;

use super::resample::{self, Resampler};
use super::TARGET_SAMPLE_RATE;

/// Audio handler that receives CMSampleBuffer callbacks from ScreenCaptureKit
/// and sends PCM data through a channel.
struct AudioHandler {
    sender: mpsc::Sender<Vec<u8>>,
    /// Shared filtered resampler (48 kHz -> 16 kHz); Mutex because the
    /// ScreenCaptureKit callback borrows `&self`.
    resampler: Mutex<Resampler>,
    /// Channel count the stream was configured with — used to mix down a
    /// single interleaved buffer.
    channel_count: usize,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, output_type: SCStreamOutputType) {
        match output_type {
            SCStreamOutputType::Audio => {
                if let Some(audio_buffer_list) = sample.audio_buffer_list() {
                    // ScreenCaptureKit with stereo config may deliver audio as:
                    // - N separate mono buffers (deinterleaved L/R), OR
                    // - 1 interleaved buffer with `channel_count` channels
                    // Mix all channels down to a proper mono average.
                    let mut mono: Option<Vec<f32>> = None;
                    let mut n_buffers = 0usize;

                    for audio_buffer in audio_buffer_list.into_iter() {
                        let raw_data = audio_buffer.data();
                        if raw_data.is_empty() {
                            continue;
                        }
                        let samples: &[f32] = unsafe {
                            std::slice::from_raw_parts(
                                raw_data.as_ptr() as *const f32,
                                raw_data.len() / 4,
                            )
                        };
                        n_buffers += 1;
                        match &mut mono {
                            None => mono = Some(samples.to_vec()),
                            Some(acc) => {
                                for (a, &b) in acc.iter_mut().zip(samples.iter()) {
                                    *a += b;
                                }
                            }
                        }
                    }

                    let Some(mut mono) = mono else { return };

                    if n_buffers > 1 {
                        // Deinterleaved: we summed the per-channel buffers
                        for s in &mut mono {
                            *s /= n_buffers as f32;
                        }
                    } else {
                        // Single interleaved buffer
                        mono = resample::mixdown_f32(&mono, self.channel_count);
                    }

                    // Filtered decimation to TARGET_SAMPLE_RATE (anti-aliased),
                    // then pack to PCM s16le.
                    let resampled = self
                        .resampler
                        .lock()
                        .map(|mut r| r.process(&mono))
                        .unwrap_or_default();
                    let pcm_s16 = resample::f32_to_s16le(&resampled);

                    if !pcm_s16.is_empty() {
                        let _ = self.sender.send(pcm_s16);
                    }
                }
            }
            _ => {
                // Ignore video frames
            }
        }
    }
}

/// System audio capture using ScreenCaptureKit
/// Captures all system audio output and converts to PCM s16le 16kHz mono.
pub struct SystemAudioCapture {
    is_capturing: Arc<AtomicBool>,
}

impl SystemAudioCapture {
    pub fn new() -> Self {
        Self {
            is_capturing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start capturing system audio.
    /// Returns a receiver that yields PCM s16le 16kHz mono audio chunks.
    pub fn start(&self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        if self.is_capturing.load(Ordering::SeqCst) {
            return Err("Already capturing".to_string());
        }

        // Get available displays
        let content = SCShareableContent::get().map_err(|e| {
            format!(
                "Failed to get shareable content (Screen Recording permission needed): {}",
                e
            )
        })?;

        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or("No displays found".to_string())?;

        // Create content filter for the main display
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();

        // Configure: audio only, 48kHz stereo (ScreenCaptureKit native rate)
        // Downsampling to 16kHz mono happens in AudioHandler
        let config = SCStreamConfiguration::new()
            .with_width(2) // minimal video (required by API)
            .with_height(2)
            .with_captures_audio(true)
            .with_excludes_current_process_audio(true) // Prevent TTS audio feedback loop
            .with_sample_rate(48000)
            .with_channel_count(2);

        // Create channel for audio data
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();

        let handler = AudioHandler {
            sender,
            resampler: Mutex::new(Resampler::new(48000, TARGET_SAMPLE_RATE)),
            channel_count: 2,
        };

        // Create and start the stream
        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(handler, SCStreamOutputType::Audio);

        stream
            .start_capture()
            .map_err(|e| format!("Failed to start system audio capture: {}", e))?;

        self.is_capturing.store(true, Ordering::SeqCst);

        // Keep the stream alive in a background thread
        let is_capturing = self.is_capturing.clone();
        std::thread::spawn(move || {
            while is_capturing.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let _ = stream.stop_capture();
        });

        Ok(receiver)
    }

    /// Stop capturing
    pub fn stop(&self) {
        self.is_capturing.store(false, Ordering::SeqCst);
    }
}

impl Default for SystemAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}
