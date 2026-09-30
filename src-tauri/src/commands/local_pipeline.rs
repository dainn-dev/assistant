use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::ipc::{Channel, InvokeBody, Request};

/// State for the local pipeline sidecar process
pub struct LocalPipelineState {
    pub process: Mutex<Option<Child>>,
}

fn log_to_file(msg: &str) {
    use std::fs::OpenOptions;
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("myjavis_pipeline.log"))
        .and_then(|mut f| writeln!(f, "[{}] {}", chrono_now(), msg));
    tracing::info!("[local-pipeline] {}", msg);
}

/// Home directory of the current user (cross-platform).
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default()
}

/// Path of the MLX venv python installed by scripts/setup_mlx.py.
/// The setup script is macOS-only (~/Library/Application Support/MyJavis),
/// so on other platforms this path simply won't exist and we fall back
/// to the system python.
fn mlx_venv_python() -> PathBuf {
    home_dir().join("Library/Application Support/MyJavis/mlx-env/bin/python3")
}

/// Candidate python executable for the pipeline sidecar.
fn python_executable() -> String {
    let venv = mlx_venv_python();
    if venv.exists() {
        log_to_file(&format!("Using venv python: {}", venv.display()));
        return venv.to_string_lossy().to_string();
    }
    #[cfg(target_os = "macos")]
    {
        if std::path::Path::new("/opt/homebrew/bin/python3").exists() {
            log_to_file("Using homebrew python");
            return "/opt/homebrew/bin/python3".to_string();
        }
        "python3".to_string()
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows: python is the conventional launcher name.
        "python".to_string()
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", now)
}

/// Start the local translation pipeline (Python sidecar)
#[tauri::command]
pub fn start_local_pipeline(
    source_lang: String,
    target_lang: String,
    channel: Channel<String>,
    state: tauri::State<'_, LocalPipelineState>,
) -> Result<(), String> {
    log_to_file(&format!(
        "start_local_pipeline called: src={}, tgt={}",
        source_lang, target_lang
    ));

    // Send status to frontend
    let _ = channel.send(r#"{"type":"status","message":"Stopping old pipeline..."}"#.to_string());

    // Stop existing pipeline
    stop_local_pipeline_inner(&state);

    let _ = channel.send(r#"{"type":"status","message":"Finding pipeline script..."}"#.to_string());

    // Find the Python script — try multiple locations
    let script_path = {
        let candidates = vec![
            // Dev: project root (when running from src-tauri/)
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/local_pipeline.py"),
            // Dev: relative to current working directory
            std::path::PathBuf::from("scripts/local_pipeline.py"),
            // Production: relative to executable
            std::env::current_exe()
                .unwrap_or_default()
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("../Resources/scripts/local_pipeline.py"),
        ];

        log_to_file(&format!(
            "Checking candidates: {:?}",
            candidates
                .iter()
                .map(|p| format!("{:?} exists={}", p, p.exists()))
                .collect::<Vec<_>>()
        ));

        candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
            "Pipeline script not found. Ensure scripts/local_pipeline.py exists.".to_string()
        })?
    };

    log_to_file(&format!("Using script: {:?}", script_path));
    let _ =
        channel.send(r#"{"type":"status","message":"Starting Python pipeline..."}"#.to_string());

    // Kill orphaned pipeline processes left behind by a crashed run.
    // Scoped to macOS (pkill does not exist on Windows) and matched against
    // the resolved script path rather than a bare substring.
    #[cfg(target_os = "macos")]
    {
        let pattern = script_path.to_string_lossy().to_string();
        let _ = Command::new("pkill").args(["-f", &pattern]).output();
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    let python = python_executable();

    let mut cmd = Command::new(&python);
    cmd.arg(&script_path)
        .arg("--asr-model")
        .arg("whisper")
        .arg("--source-lang")
        .arg(&source_lang)
        .arg("--target-lang")
        .arg(&target_lang)
        .env("TOKENIZERS_PARALLELISM", "false");

    // macOS: prepend homebrew paths (tauri spawns with a minimal PATH).
    // Other platforms inherit the parent PATH unchanged.
    #[cfg(target_os = "macos")]
    {
        cmd.env("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        cmd.env("HOME", home_dir());
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let msg = format!("Failed to start pipeline: {}", e);
            log_to_file(&msg);
            msg
        })?;

    log_to_file(&format!("Python process spawned, PID={}", child.id()));
    let _ = channel.send(format!(
        r#"{{"type":"status","message":"Python started (PID={}), loading models..."}}"#,
        child.id()
    ));

    // Read stdout in a background thread and forward JSON to frontend
    let stdout = child.stdout.take().ok_or("Failed to get stdout")?;

    let stderr = child.stderr.take().ok_or("Failed to get stderr")?;

    // Forward stdout (JSON results) to frontend
    let channel_clone = channel.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.is_empty() => {
                    log_to_file(&format!("stdout: {}", line));
                    let _ = channel_clone.send(line);
                }
                Err(e) => {
                    log_to_file(&format!("stdout error: {}", e));
                    break;
                }
                _ => {}
            }
        }
        log_to_file("stdout reader ended");
    });

    // Log stderr AND forward to frontend as status
    let channel_clone2 = channel.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    log_to_file(&format!("stderr: {}", line));
                    // Forward pipeline status to frontend
                    let escaped = line.replace('"', r#"\""#);
                    let _ = channel_clone2
                        .send(format!(r#"{{"type":"status","message":"{}"}}"#, escaped));
                }
                Err(_) => break,
            }
        }
        log_to_file("stderr reader ended");
    });

    let mut proc = state.process.lock().map_err(|e| e.to_string())?;
    *proc = Some(child);

    log_to_file("Pipeline state saved, returning OK");
    Ok(())
}

/// Send audio data to the local pipeline stdin.
///
/// Accepts a raw binary IPC body (`invoke('send_audio_to_pipeline', bytes)`)
/// so PCM frames don't cross the bridge as JSON number arrays (~10x bloat).
/// On Android raw bodies aren't supported, so a JSON array body is accepted
/// as a fallback.
#[tauri::command]
pub fn send_audio_to_pipeline(
    request: Request<'_>,
    state: tauri::State<'_, LocalPipelineState>,
) -> Result<(), String> {
    let data: Vec<u8> = match request.body() {
        InvokeBody::Raw(bytes) => bytes.clone(),
        InvokeBody::Json(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u8))
            .collect(),
        _ => return Err("Expected raw PCM bytes body".to_string()),
    };

    let mut proc = state.process.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut child) = *proc {
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(&data).map_err(|e| {
                log_to_file(&format!("stdin write error: {}", e));
                e.to_string()
            })?;
            stdin.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Stop the local pipeline
#[tauri::command]
pub fn stop_local_pipeline(state: tauri::State<'_, LocalPipelineState>) -> Result<(), String> {
    log_to_file("stop_local_pipeline called");
    stop_local_pipeline_inner(&state);
    Ok(())
}

fn stop_local_pipeline_inner(state: &LocalPipelineState) {
    if let Ok(mut proc) = state.process.lock() {
        if let Some(mut child) = proc.take() {
            log_to_file(&format!("Killing pipeline PID={}", child.id()));
            // Close stdin to signal the pipeline to stop
            drop(child.stdin.take());
            // Give it a moment, then kill if needed
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = child.kill();
            let _ = child.wait();
            log_to_file("Pipeline killed");
        }
    }
}

/// Check if MLX setup is complete
#[tauri::command]
pub fn check_mlx_setup() -> Result<String, String> {
    let venv_python = mlx_venv_python();
    let marker = venv_python
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(".setup_complete");

    if marker.exists() && venv_python.exists() {
        // Read marker to get details
        let content = std::fs::read_to_string(&marker).unwrap_or_default();
        Ok(format!(
            r#"{{"ready":true,"python":"{}","details":{}}}"#,
            venv_python.to_string_lossy().replace('\\', "/"),
            content
        ))
    } else {
        Ok(r#"{"ready":false}"#.to_string())
    }
}

/// Run MLX setup (install venv + packages + download models)
#[tauri::command]
pub fn run_mlx_setup(channel: Channel<String>) -> Result<(), String> {
    log_to_file("run_mlx_setup called");

    // Find setup script
    let script_path = {
        let candidates = vec![
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/setup_mlx.py"),
            std::path::PathBuf::from("scripts/setup_mlx.py"),
            std::env::current_exe()
                .unwrap_or_default()
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("../Resources/scripts/setup_mlx.py"),
        ];

        candidates
            .into_iter()
            .find(|p| p.exists())
            .ok_or_else(|| "Setup script not found.".to_string())?
    };

    // Use system python to run setup (which creates the venv).
    // Do not use the venv python itself — it doesn't exist yet.
    #[cfg(target_os = "macos")]
    let python = if std::path::Path::new("/opt/homebrew/bin/python3").exists() {
        "/opt/homebrew/bin/python3"
    } else {
        "python3"
    };
    #[cfg(not(target_os = "macos"))]
    let python = "python";

    let mut child = Command::new(python)
        .arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start setup: {}", e))?;

    log_to_file(&format!("Setup process spawned, PID={}", child.id()));

    // Forward stdout (JSON progress) to frontend
    let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
    let channel_clone = channel.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) if !line.is_empty() => {
                    log_to_file(&format!("setup stdout: {}", line));
                    let _ = channel_clone.send(line);
                }
                Err(e) => {
                    log_to_file(&format!("setup stdout error: {}", e));
                    break;
                }
                _ => {}
            }
        }
    });

    // Forward stderr to log + frontend
    let stderr = child.stderr.take().ok_or("Failed to get stderr")?;
    let channel_clone2 = channel.clone();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    log_to_file(&format!("setup stderr: {}", line));
                    let escaped = line.replace('"', r#"\""#);
                    let _ =
                        channel_clone2.send(format!(r#"{{"type":"log","message":"{}"}}"#, escaped));
                }
                Err(_) => break,
            }
        }
    });

    Ok(())
}
