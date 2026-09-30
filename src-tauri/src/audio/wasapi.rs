use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use super::resample::{self, Resampler};
use super::TARGET_SAMPLE_RATE;

/// System audio capture using WASAPI loopback on Windows.
/// Captures all system audio output and converts to PCM s16le 16kHz mono.
/// On Windows 10 20H2+, uses Application Loopback API (ALAC) to exclude own process audio.
/// Falls back to legacy WASAPI loopback on older systems.
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

        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let is_capturing = self.is_capturing.clone();
        is_capturing.store(true, Ordering::SeqCst);

        let own_pid = std::process::id();

        std::thread::spawn(move || {
            // Try ALAC first (Windows 10 20H2+ — excludes own process audio)
            match start_app_loopback(own_pid, sender.clone(), is_capturing.clone()) {
                Ok(()) => {
                    // ALAC path completed normally
                }
                Err(e) => {
                    tracing::warn!(
                        "[wasapi] Application Loopback (ALAC) not available: {}. \
                         Falling back to legacy WASAPI loopback.",
                        e
                    );
                    start_legacy_loopback(sender, is_capturing);
                }
            }
        });

        Ok(receiver)
    }

    /// Stop capturing
    pub fn stop(&self) {
        self.is_capturing.store(false, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn is_capturing(&self) -> bool {
        self.is_capturing.load(Ordering::SeqCst)
    }
}

impl Default for SystemAudioCapture {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Windows API imports
// ─────────────────────────────────────────────────────────────────────────────

use windows::core::{implement, IUnknown, Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, ActivateAudioInterfaceAsync,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

// ─────────────────────────────────────────────────────────────────────────────
// COM completion handler for ActivateAudioInterfaceAsync
// ─────────────────────────────────────────────────────────────────────────────

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler {
    sender: mpsc::SyncSender<Result<IAudioClient, String>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler_Impl {
    fn ActivateCompleted(
        &self,
        activate_operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let result: Result<IAudioClient, String> = (|| {
            let op = activate_operation.ok_or_else(|| "No operation provided".to_string())?;
            let mut hr = windows::core::HRESULT(0);
            let mut activated: Option<IUnknown> = None;
            unsafe {
                op.GetActivateResult(&mut hr, &mut activated)
                    .map_err(|e| e.to_string())?;
            }
            hr.ok().map_err(|e| e.to_string())?;
            let client: IAudioClient = activated
                .ok_or_else(|| "No audio client returned".to_string())?
                .cast()
                .map_err(|e| e.to_string())?;
            Ok(client)
        })();
        let _ = self.sender.send(result);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Build a VT_BLOB PROPVARIANT pointing at a CoTaskMem-allocated copy of
// AUDIOCLIENT_ACTIVATION_PARAMS.
//
// windows::core::PROPVARIANT is repr(transparent) over a private inner type.
// Layout (matching the Windows SDK PROPVARIANT structure):
//   offset 0: vt (u16)
//   offset 2: wReserved1/2/3 (u16 x3)
//   offset 8: cbSize (u32)  [blob.cbSize]
//   offset 16: pBlobData (*mut u8) [blob.pBlobData, 8-byte aligned on 64-bit]
//
// VT_BLOB = 65 (0x41)
//
// The blob is allocated with CoTaskMemAlloc, so when the returned PROPVARIANT
// is dropped, PropVariantClear frees it via CoTaskMemFree — no manual free and
// no leak. The PROPVARIANT must stay alive until activation completes.
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn build_activation_propvariant(
    params: &AUDIOCLIENT_ACTIVATION_PARAMS,
) -> Result<windows::core::PROPVARIANT, String> {
    const VT_BLOB: u16 = 65u16; // VT_BLOB value from propidlbase.h
    let size = std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>();
    let mem = CoTaskMemAlloc(size) as *mut u8;
    if mem.is_null() {
        return Err("CoTaskMemAlloc failed for activation params".to_string());
    }
    std::ptr::copy_nonoverlapping(params as *const _ as *const u8, mem, size);

    // PROPVARIANT is repr(transparent) wrapping the inner raw struct.
    // Both share the same memory layout, so we can zero-init via PROPVARIANT
    // and then write into its bytes directly via pointer arithmetic.
    let mut pv = windows::core::PROPVARIANT::new();
    let pv_ptr = &mut pv as *mut windows::core::PROPVARIANT as *mut u8;

    // vt field is at offset 0, size 2
    (pv_ptr as *mut u16).write(VT_BLOB);

    // blob.cbSize is at offset 8 on both 32-bit and 64-bit Windows
    // (vt:u16, wReserved1:u16, wReserved2:u16, wReserved3:u16 = 8 bytes total padding)
    let cbsize_ptr = pv_ptr.add(8) as *mut u32;
    cbsize_ptr.write(size as u32);

    // blob.pBlobData is at offset 16 on 64-bit (cbSize:u32 + 4 bytes padding + ptr)
    // On 32-bit it would be at offset 12, but Tauri targets 64-bit Windows.
    let pblobdata_ptr = pv_ptr.add(16) as *mut *mut u8;
    pblobdata_ptr.write(mem);

    Ok(pv)
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared capture loop (reused by both ALAC and legacy paths)
// ─────────────────────────────────────────────────────────────────────────────

unsafe fn run_capture_loop(
    audio_client: &IAudioClient,
    sender: &mpsc::Sender<Vec<u8>>,
    is_capturing: &Arc<AtomicBool>,
    source_channels: u32,
    bits_per_sample: u16,
    resampler: &mut Resampler,
) {
    let capture_client: IAudioCaptureClient = match audio_client.GetService() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("[wasapi] Failed to get capture client: {}", e);
            return;
        }
    };

    if let Err(e) = audio_client.Start() {
        tracing::error!("[wasapi] Failed to start audio client: {}", e);
        return;
    }

    while is_capturing.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));

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
                let pcm_data = convert_to_pcm_s16_16k(
                    buffer_ptr,
                    num_frames,
                    source_channels,
                    bits_per_sample,
                    resampler,
                );

                if !pcm_data.is_empty() && sender.send(pcm_data).is_err() {
                    break; // Receiver dropped
                }
            }
        }

        let _ = capture_client.ReleaseBuffer(num_frames);
    }

    let _ = audio_client.Stop();
}

// ─────────────────────────────────────────────────────────────────────────────
// ALAC path: Windows 10 20H2+ — excludes own PID from captured audio
// ─────────────────────────────────────────────────────────────────────────────

fn start_app_loopback(
    own_pid: u32,
    sender: mpsc::Sender<Vec<u8>>,
    is_capturing: Arc<AtomicBool>,
) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        // Build activation params to exclude own process
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: own_pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };

        // params is memcpy'd into the blob; only prop_variant must outlive activation
        let prop_variant = build_activation_propvariant(&params)?;

        // Sync channel for async completion handler
        let (tx, rx) = mpsc::sync_channel::<Result<IAudioClient, String>>(1);
        let handler = CompletionHandler { sender: tx };
        let handler_com: IActivateAudioInterfaceCompletionHandler = handler.into();

        // VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK = "VAD\\Process_Loopback"
        let device_id: PCWSTR = windows::core::w!("VAD\\Process_Loopback");

        ActivateAudioInterfaceAsync(
            device_id,
            &IAudioClient::IID,
            Some(&prop_variant),
            &handler_com,
        )
        .map_err(|e| format!("ActivateAudioInterfaceAsync failed: {}", e))?;

        // Wait for async activation to complete (timeout = 5s)
        let audio_client = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "Timeout waiting for ALAC audio activation".to_string())??;

        // Get the mix format to determine source rate/channels
        let mix_format_ptr = audio_client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat failed: {}", e))?;
        let mix_format = &*mix_format_ptr;

        let source_rate = mix_format.nSamplesPerSec;
        let source_channels = mix_format.nChannels as u32;
        let bits_per_sample = mix_format.wBitsPerSample;

        // ALAC is already a loopback stream — no AUDCLNT_STREAMFLAGS_LOOPBACK needed
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

        tracing::info!(
            "[wasapi] Using Application Loopback (process exclusion) — own PID {} excluded",
            own_pid
        );

        let mut resampler = Resampler::new(source_rate, TARGET_SAMPLE_RATE);

        run_capture_loop(
            &audio_client,
            &sender,
            &is_capturing,
            source_channels,
            bits_per_sample,
            &mut resampler,
        );

        CoUninitialize();
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Legacy WASAPI loopback path (captures all system audio, no exclusion)
// ─────────────────────────────────────────────────────────────────────────────

fn start_legacy_loopback(sender: mpsc::Sender<Vec<u8>>, is_capturing: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("[wasapi] Failed to create device enumerator: {}", e);
                    return;
                }
            };

        let device = match enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("[wasapi] Failed to get default audio endpoint: {}", e);
                return;
            }
        };

        let audio_client: IAudioClient = match device.Activate(CLSCTX_ALL, None) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("[wasapi] Failed to activate audio client: {}", e);
                return;
            }
        };

        let mix_format_ptr = match audio_client.GetMixFormat() {
            Ok(f) => f,
            Err(e) => {
                tracing::error!("[wasapi] Failed to get mix format: {}", e);
                return;
            }
        };
        let mix_format = &*mix_format_ptr;

        let source_rate = mix_format.nSamplesPerSec;
        let source_channels = mix_format.nChannels as u32;
        let bits_per_sample = mix_format.wBitsPerSample;

        if let Err(e) = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            mix_format_ptr,
            None,
        ) {
            tracing::error!(
                "[wasapi] Failed to initialize audio client in loopback mode: {}",
                e
            );
            return;
        }

        tracing::info!("[wasapi] Using legacy WASAPI loopback (no process exclusion)");

        let mut resampler = Resampler::new(source_rate, TARGET_SAMPLE_RATE);

        run_capture_loop(
            &audio_client,
            &sender,
            &is_capturing,
            source_channels,
            bits_per_sample,
            &mut resampler,
        );

        CoUninitialize();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PCM conversion helper
// ─────────────────────────────────────────────────────────────────────────────

/// Convert raw WASAPI buffer to PCM s16le 16kHz mono.
/// Mixes all channels down to mono (average) and uses the shared filtered
/// resampler so audio above 8 kHz is attenuated instead of aliasing.
unsafe fn convert_to_pcm_s16_16k(
    buffer_ptr: *mut u8,
    num_frames: u32,
    source_channels: u32,
    bits_per_sample: u16,
    resampler: &mut Resampler,
) -> Vec<u8> {
    let frame_count = num_frames as usize;

    // Read samples as f32 (WASAPI typically delivers IEEE float)
    let f32_samples = if bits_per_sample == 32 {
        let ptr = buffer_ptr as *const f32;
        std::slice::from_raw_parts(ptr, frame_count * source_channels as usize)
    } else {
        return Vec::new(); // Unsupported format
    };

    let mono = resample::mixdown_f32(f32_samples, source_channels as usize);
    resample::f32_to_s16le(&resampler.process(&mono))
}

/// Probe whether a default capture (microphone) device exists.
/// Used by `check_permissions` — device presence is the practical failure
/// mode on Windows; the OS-level mic privacy toggle can't be queried per-app.
pub fn default_input_device_present() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(_) => return false,
            };
        enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .is_ok()
    }
}
