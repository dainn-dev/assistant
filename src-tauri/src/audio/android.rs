use std::sync::mpsc;
use std::sync::OnceLock;

// === Android System Audio (MediaProjection) ===
//
// Microphone capture lives in `cpal_mic.rs` (shared with macOS/Linux).
// The Kotlin MediaProjectionService pushes PCM s16le 16kHz mono bytes
// through `onPcmData` below; this module only brokers the channel.

static SYSTEM_TX: OnceLock<mpsc::Sender<Vec<u8>>> = OnceLock::new();

pub struct SystemAudioCapture;

impl SystemAudioCapture {
    pub fn new() -> Self {
        Self
    }

    /// Returns a receiver that will get PCM s16le 16kHz mono frames from the Kotlin service.
    pub fn start(&self) -> Result<mpsc::Receiver<Vec<u8>>, String> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let _ = SYSTEM_TX.set(tx);
        Ok(rx)
    }

    pub fn stop(&self) {
        // Best-effort stop on Android; also clears our sender so JNI drops data.
        let _ = stop_media_projection_service();
        // There's no stable way to "unset" OnceLock; dropping the service is what matters.
    }
}

pub fn request_media_projection() -> Result<(), String> {
    android_call_static_with_activity(
        "com/personal/translator/MediaProjectionActivity",
        "request",
        "(Landroid/app/Activity;)V",
    )
}

pub fn stop_media_projection_service() -> Result<(), String> {
    android_call_static_with_activity(
        "com/personal/translator/MediaProjectionService",
        "stop",
        "(Landroid/app/Activity;)V",
    )
}

#[cfg(target_os = "android")]
fn android_call_static_with_activity(
    class_path: &str,
    method: &str,
    sig: &str,
) -> Result<(), String> {
    use jni::objects::{JObject, JValue};
    use jni::JavaVM;

    let ctx = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(ctx.vm() as *mut jni::sys::JavaVM) }
        .map_err(|e| format!("Android VM: {e}"))?;
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| format!("Android attach thread: {e}"))?;

    let activity = unsafe { JObject::from_raw(ctx.context() as jni::sys::jobject) };
    let activity_global = env
        .new_global_ref(activity)
        .map_err(|e| format!("Android global ref: {e}"))?;

    let cls = env
        .find_class(class_path)
        .map_err(|e| format!("find_class({class_path}): {e}"))?;
    env.call_static_method(
        cls,
        method,
        sig,
        &[JValue::Object(activity_global.as_obj())],
    )
    .map_err(|e| format!("call_static_method({method}): {e}"))?;

    Ok(())
}

/// Called from Kotlin `MediaProjectionService` to push PCM bytes into Rust.
///
/// Signature must match: `com.personal.translator.MediaProjectionService.onPcmData(byte[], int)`
#[no_mangle]
pub extern "system" fn Java_com_personal_translator_MediaProjectionService_onPcmData(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    data: jni::sys::jbyteArray,
    len: jni::sys::jint,
) {
    let Some(tx) = SYSTEM_TX.get() else {
        return;
    };
    if len <= 0 {
        return;
    }

    use jni::objects::JByteArray;

    let byte_len = len as usize;
    let arr = unsafe { JByteArray::from_raw(data) };
    let Ok(mut bytes) = env.convert_byte_array(arr) else {
        return;
    };

    // Kotlin always passes a scratch buffer; we only want the valid prefix.
    if bytes.len() > byte_len {
        bytes.truncate(byte_len);
    }

    let _ = tx.send(bytes);
}
