use std::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::TARGET_SAMPLE_RATE;

/// System audio capture on Linux via PulseAudio/PipeWire monitor source.
/// Records from @DEFAULT_MONITOR@ which captures all system audio output.
pub struct SystemAudioCapture {
    is_capturing: Arc<AtomicBool>,
}

impl SystemAudioCapture {
    pub fn new() -> Self {
        Self {
            is_capturing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        if self.is_capturing.load(Ordering::SeqCst) {
            return Err("Already capturing".to_string());
        }

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let is_capturing = self.is_capturing.clone();
        is_capturing.store(true, Ordering::SeqCst);

        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        std::thread::spawn(move || {
            use libpulse_binding as pulse;
            use libpulse_simple_binding::Simple;
            use pulse::sample::{Format, Spec};
            use pulse::stream::Direction;

            let spec = Spec {
                format: Format::S16le,
                rate: TARGET_SAMPLE_RATE,
                channels: 1,
            };

            let simple = match Simple::new(
                None,
                "MyJavis",
                Direction::Record,
                Some("@DEFAULT_MONITOR@"),
                "System Audio",
                &spec,
                None,
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("PulseAudio error: {}", e)));
                    is_capturing.store(false, Ordering::SeqCst);
                    return;
                }
            };

            let _ = ready_tx.send(Ok(()));

            // 50ms chunk: 16000 Hz * 2 bytes/sample * 0.05s = 1600 bytes
            let mut buf = vec![0u8; 1600];
            while is_capturing.load(Ordering::SeqCst) {
                match simple.read(&mut buf) {
                    Ok(()) => {
                        let _ = tx.send(buf.clone());
                    }
                    Err(_) => break,
                }
            }

            is_capturing.store(false, Ordering::SeqCst);
        });

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(rx),
            Ok(Err(e)) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                Err(e)
            }
            Err(_) => {
                self.is_capturing.store(false, Ordering::SeqCst);
                Err("System audio worker failed to start".to_string())
            }
        }
    }

    pub fn stop(&self) {
        self.is_capturing.store(false, Ordering::SeqCst);
    }
}

impl Default for SystemAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}
