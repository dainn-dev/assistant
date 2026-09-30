use crate::audio::MicCapture;
use crate::audio::SystemAudioCapture;
use serde::Serialize;
use std::sync::mpsc;
use std::sync::Mutex;
use tauri::{ipc::Channel, State};

/// State for tracking active audio captures
pub struct AudioState {
    pub system_audio: Mutex<SystemAudioCapture>,
    pub microphone: Mutex<MicCapture>,
    pub active_receiver: Mutex<Vec<AudioForwarder>>,
}

/// Forwards audio from a receiver to a Tauri IPC channel
pub struct AudioForwarder {
    /// Handle to signal stop
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AudioForwarder {
    fn stop(&self) {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Serialize, Clone)]
pub struct PermissionStatus {
    pub screen_recording: String,
    pub microphone: String,
}

/// Request MediaProjection permission on Android (no-op on desktop).
#[tauri::command]
pub fn request_media_projection() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        crate::audio::android::request_media_projection()
    }
    #[cfg(not(target_os = "android"))]
    {
        Ok(())
    }
}

/// Start audio capture and forward data to the frontend via IPC channel
#[tauri::command]
pub fn start_capture(
    source: String,
    channel: Channel<Vec<u8>>,
    state: State<'_, AudioState>,
) -> Result<(), String> {
    // Stop any existing capture first
    stop_capture_inner(&state);

    let receiver: mpsc::Receiver<Vec<u8>> = match source.as_str() {
        "system" => {
            let sys = state
                .system_audio
                .lock()
                .map_err(|_| "Lock error".to_string())?;
            sys.start()?
        }
        "microphone" => {
            let mut mic = state
                .microphone
                .lock()
                .map_err(|_| "Lock error".to_string())?;
            mic.start()?
        }
        "both" => {
            // Start both sources and merge into a single receiver
            let sys = state
                .system_audio
                .lock()
                .map_err(|_| "Lock error".to_string())?;
            let sys_rx = sys.start()?;
            let mut mic = state
                .microphone
                .lock()
                .map_err(|_| "Lock error".to_string())?;
            let mic_rx = mic.start()?;

            let (merged_tx, merged_rx) = mpsc::channel::<Vec<u8>>();
            let tx1 = merged_tx.clone();
            let tx2 = merged_tx;

            // Forward system audio to merged channel
            std::thread::spawn(move || {
                while let Ok(data) = sys_rx.recv() {
                    if tx1.send(data).is_err() {
                        break;
                    }
                }
            });
            // Forward mic audio to merged channel
            std::thread::spawn(move || {
                while let Ok(data) = mic_rx.recv() {
                    if tx2.send(data).is_err() {
                        break;
                    }
                }
            });

            merged_rx
        }
        _ => return Err(format!("Unknown source: {}", source)),
    };

    register_forwarder(&state, receiver, move |data| {
        channel.send(data).map_err(|_| ())
    })?;
    Ok(())
}

/// Spawn a batching forwarder thread: buffers PCM chunks and flushes every
/// 200 ms or when the receiver disconnects / the stop flag fires.
fn spawn_forwarder(
    receiver: mpsc::Receiver<Vec<u8>>,
    send: impl Fn(Vec<u8>) -> Result<(), ()> + Send + 'static,
) -> AudioForwarder {
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag_clone = stop_flag.clone();

    std::thread::spawn(move || {
        let mut buffer: Vec<u8> = Vec::with_capacity(32000); // ~1 sec at 16kHz s16le
        let batch_interval = std::time::Duration::from_millis(200);
        let mut last_flush = std::time::Instant::now();

        loop {
            if stop_flag_clone.load(std::sync::atomic::Ordering::SeqCst) {
                // Flush remaining buffer before exit
                if !buffer.is_empty() {
                    let _ = send(buffer.clone());
                }
                break;
            }

            match receiver.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(data) => {
                    buffer.extend_from_slice(&data);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if !buffer.is_empty() {
                        let _ = send(buffer.clone());
                    }
                    break;
                }
            }

            // Flush buffer every 200ms
            if last_flush.elapsed() >= batch_interval && !buffer.is_empty() {
                if send(buffer.clone()).is_err() {
                    break; // Channel closed
                }
                buffer.clear();
                last_flush = std::time::Instant::now();
            }
        }
    });

    AudioForwarder { stop_flag }
}

/// Register a forwarder on the shared state so `stop_capture` can halt it.
fn register_forwarder(
    state: &AudioState,
    receiver: mpsc::Receiver<Vec<u8>>,
    send: impl Fn(Vec<u8>) -> Result<(), ()> + Send + 'static,
) -> Result<(), String> {
    let forwarder = spawn_forwarder(receiver, send);
    let mut active = state
        .active_receiver
        .lock()
        .map_err(|_| "Lock error".to_string())?;
    active.push(forwarder);
    Ok(())
}

/// Start System Audio and Microphone captures on separate IPC channels.
///
/// Unlike `start_capture("both")`, the two PCM streams are never merged, so
/// the frontend can feed two independent recognition clients and always know
/// which physical source produced which transcript.
#[tauri::command]
pub fn start_split_capture(
    system_channel: Channel<Vec<u8>>,
    mic_channel: Channel<Vec<u8>>,
    state: State<'_, AudioState>,
) -> Result<(), String> {
    // Stop any existing capture first
    stop_capture_inner(&state);

    let sys_rx = {
        let sys = state
            .system_audio
            .lock()
            .map_err(|_| "Lock error".to_string())?;
        sys.start().map_err(|e| format!("system audio: {}", e))?
    };

    let mic_rx = {
        let mut mic = state
            .microphone
            .lock()
            .map_err(|_| "Lock error".to_string())?;
        match mic.start() {
            Ok(rx) => rx,
            Err(e) => {
                // Roll back the already-started system capture so a retry
                // doesn't fight a dangling loopback stream.
                if let Ok(sys) = state.system_audio.lock() {
                    sys.stop();
                }
                return Err(format!("microphone: {}", e));
            }
        }
    };

    register_forwarder(&state, sys_rx, move |data| {
        system_channel.send(data).map_err(|_| ())
    })?;
    register_forwarder(&state, mic_rx, move |data| {
        mic_channel.send(data).map_err(|_| ())
    })?;

    Ok(())
}

/// Stop audio capture
#[tauri::command]
pub fn stop_capture(state: State<'_, AudioState>) -> Result<(), String> {
    stop_capture_inner(&state);
    Ok(())
}

fn stop_capture_inner(state: &AudioState) {
    // Stop all active forwarders
    if let Ok(mut active) = state.active_receiver.lock() {
        for forwarder in active.drain(..) {
            forwarder.stop();
        }
    }

    // Stop system audio
    if let Ok(sys) = state.system_audio.lock() {
        sys.stop();
    }

    // Stop microphone
    if let Ok(mut mic) = state.microphone.lock() {
        mic.stop();
    }
}

/// Check audio capture permissions.
///
/// Probes what is actually knowable per platform:
/// - Windows: WASAPI loopback needs no consent; mic reports whether a default
///   capture device exists (the OS mic-privacy toggle can't be queried
///   per-app — a denied stream fails at capture time with a visible error).
/// - macOS: `SCShareableContent::get()` fails when Screen Recording is denied;
///   mic reports input-device presence (TCC prompt fires on first stream open).
/// - Android/Linux: presence probes where meaningful, otherwise "unknown".
#[tauri::command]
pub fn check_permissions() -> PermissionStatus {
    #[cfg(target_os = "windows")]
    {
        PermissionStatus {
            screen_recording: "granted".to_string(),
            microphone: if crate::audio::wasapi::default_input_device_present() {
                "granted".to_string()
            } else {
                "denied".to_string()
            },
        }
    }
    #[cfg(target_os = "macos")]
    {
        use cpal::traits::HostTrait;
        PermissionStatus {
            screen_recording: match screencapturekit::prelude::SCShareableContent::get() {
                Ok(_) => "granted".to_string(),
                Err(_) => "denied".to_string(),
            },
            microphone: match cpal::default_host().default_input_device() {
                Some(_) => "granted".to_string(),
                None => "denied".to_string(),
            },
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        #[cfg(target_os = "android")]
        let microphone = "unknown".to_string(); // MediaProjection/record perms handled by OS dialogs
        #[cfg(not(target_os = "android"))]
        let microphone = {
            use cpal::traits::HostTrait;
            match cpal::default_host().default_input_device() {
                Some(_) => "granted".to_string(),
                None => "denied".to_string(),
            }
        };
        PermissionStatus {
            screen_recording: "unknown".to_string(),
            microphone,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_capture_registers_two_forwarders() {
        let (_tx1, rx1) = mpsc::channel::<Vec<u8>>();
        let (_tx2, rx2) = mpsc::channel::<Vec<u8>>();
        let state = AudioState {
            system_audio: Mutex::new(SystemAudioCapture::new()),
            microphone: Mutex::new(MicCapture::new()),
            active_receiver: Mutex::new(Vec::new()),
        };

        register_forwarder(&state, rx1, |_| Ok(())).unwrap();
        register_forwarder(&state, rx2, |_| Ok(())).unwrap();
        assert_eq!(state.active_receiver.lock().unwrap().len(), 2);

        stop_capture_inner(&state);
        assert!(state.active_receiver.lock().unwrap().is_empty());
    }
}
