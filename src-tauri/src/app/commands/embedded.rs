// ============================================
// Embedded OpenCode Server (Android)
//
// The Android build runs the opencode v2 server **inside the app**: a
// `node` runtime (built for bionic, shipped in the APK's native lib dir)
// executes the opencode server bundle (`opencode.mjs`) that is installed
// from the APK assets into the app's private storage.
//
// The desktop build spawns the `opencode` binary instead; the contract with
// the frontend is identical, so the same `applyLocalServiceUrl(url, password)`
// path works for both:
//
//   stdout: "server listening on http://127.0.0.1:<port>"
//           "server password <pwd>"
//
// Unlike the desktop path we always own the process lifetime: there is no
// shared service registry on a phone.
// ============================================

use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Manager, State};

/// Name of the Node runtime inside the APK's native library directory.
/// The file is a plain ELF executable; the `.so` suffix is what makes the
/// Android package manager extract it to `nativeLibraryDir` (the only place
/// an app may exec() from on API 29+).
const NODE_RUNTIME_NAME: &str = "libopencode_node.so";
/// Optional ripgrep build shipped next to the runtime. opencode's search
/// tool falls back to a system `rg` when the platform has no bundled one.
const RIPGREP_NAME: &str = "librg.so";

const PASSWORD_FILE: &str = "server-password";
const BUNDLE_DIR: &str = "opencode";
const SERVER_ENTRY: &str = "opencode.mjs";

#[derive(Default)]
pub struct EmbeddedServerState {
    /// The spawned server process.
    pub child: Mutex<Option<Child>>,
    /// PID of the process we started (0 when we do not own one).
    pub pid: AtomicU32,
    /// URL the server is listening on.
    pub url: Mutex<Option<String>>,
    /// Workspace directory the server was started in.
    pub workspace: Mutex<Option<String>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerInfo {
    /// Path of the node runtime, when it was found.
    pub runtime: Option<String>,
    /// Directory the server bundle was installed to.
    pub bundle_dir: Option<String>,
    /// Entry point of the server bundle, when it exists.
    pub entry: Option<String>,
    /// Workspace directory currently used.
    pub workspace: Option<String>,
    /// Whether a server process is currently running.
    pub running: bool,
    /// URL of the running server.
    pub url: Option<String>,
    /// Error from the last start attempt.
    pub error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedServerResult {
    pub started: bool,
    pub url: Option<String>,
    pub server_password: Option<String>,
    pub pid: Option<u32>,
    pub workspace: Option<String>,
    pub error: Option<String>,
}

// ============================================
// Paths
// ============================================

/// `nativeLibraryDir` of the running app.
///
/// Android only allows exec() from the package manager's own native library
/// directory; everything else in the app sandbox is refused for apps that
/// target API 29+. The directory is not exposed to Rust by Tauri, but the
/// process maps always contain the shared libraries loaded from it, so it can
/// be recovered from `/proc/self/maps`.
pub fn native_library_dir() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().last() else {
            continue;
        };
        if !path.ends_with(".so") {
            continue;
        }
        let path = Path::new(path);
        // /data/app/~~<hash>/<pkg>-<hash>/lib/arm64/libfoo.so -> .../lib/arm64
        let Some(dir) = path.parent() else {
            continue;
        };
        let dir_name = dir.file_name()?.to_str()?;
        if !matches!(dir_name, "arm64" | "armeabi-v7a" | "x86" | "x86_64") {
            continue;
        }
        return Some(dir.to_path_buf());
    }
    None
}

fn node_runtime_path(native_dir: &Path) -> PathBuf {
    native_dir.join(NODE_RUNTIME_NAME)
}

fn bundle_root(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    Some(dir.join(BUNDLE_DIR))
}

fn server_entry(app: &AppHandle) -> Option<PathBuf> {
    Some(bundle_root(app)?.join(SERVER_ENTRY))
}

fn home_dir(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("home"))
}

/// Password for the embedded server.
///
/// The password is generated once and persisted so the frontend can keep
/// using it across restarts (the server is started with `OPENCODE_PASSWORD`
/// and therefore never prints a random one).
fn server_password(app: &AppHandle) -> Option<String> {
    let dir = app.path().app_data_dir().ok()?;
    let path = dir.join(PASSWORD_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let generated = random_password();
    if std::fs::write(&path, &generated).is_ok() {
        Some(generated)
    } else {
        Some(generated)
    }
}

fn random_password() -> String {
    // 32 random-ish bytes rendered as hex; good enough for a loopback server
    // that is only reachable from this app.
    let mut bytes = [0u8; 32];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = file.read_exact(&mut bytes);
    } else {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes[..16].copy_from_slice(&nanos.to_le_bytes());
        bytes[16..].copy_from_slice(&(std::process::id() as u64).to_le_bytes());
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ============================================
// stdout parsing
// ============================================

/// "server listening on http://127.0.0.1:4096" -> "http://127.0.0.1:4096"
fn parse_listening_url(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("server listening on ")?;
    let url = rest
        .trim()
        .trim_end_matches(',')
        .trim_end_matches(';')
        .trim_end_matches(')');
    let url = url.replace("http://0.0.0.0:", "http://127.0.0.1:");
    let url = url.replace("http://[::]:", "http://[::1]:");
    let url = url.trim_end_matches('/');
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// "server password <pwd>" -> "<pwd>"
fn parse_server_password(line: &str) -> Option<String> {
    let password = line.trim().strip_prefix("server password ")?.trim();
    if password.is_empty() {
        None
    } else {
        Some(password.to_string())
    }
}

// ============================================
// Commands
// ============================================

#[tauri::command]
pub fn embedded_server_info(app: AppHandle, state: State<'_, EmbeddedServerState>) -> EmbeddedServerInfo {
    let native_dir = native_library_dir();
    let runtime = native_dir.as_ref().map(|dir| node_runtime_path(dir).to_string_lossy().to_string());
    let bundle_dir = bundle_root(&app).map(|p| p.to_string_lossy().to_string());
    let entry = server_entry(&app).map(|p| p.to_string_lossy().to_string());
    let entry_exists = server_entry(&app).map(|p| p.exists()).unwrap_or(false);

    EmbeddedServerInfo {
        runtime,
        bundle_dir,
        entry: if entry_exists { entry } else { None },
        workspace: state.workspace.lock().ok().and_then(|w| w.clone()),
        running: state.pid.load(Ordering::SeqCst) > 0,
        url: state.url.lock().ok().and_then(|u| u.clone()),
        error: None,
    }
}

/// Start the embedded server.
///
/// `workspace` is the directory the agent works in; it becomes the server's
/// working directory (and therefore the default project location).
#[tauri::command]
pub async fn start_embedded_server(
    app: AppHandle,
    state: State<'_, EmbeddedServerState>,
    workspace: Option<String>,
    env_vars: Option<HashMap<String, String>>,
    extra_args: Option<Vec<String>>,
) -> Result<EmbeddedServerResult, String> {
    // Already running? Reuse it.
    if state.pid.load(Ordering::SeqCst) > 0 {
        let url = state.url.lock().ok().and_then(|u| u.clone());
        return Ok(EmbeddedServerResult {
            started: false,
            url,
            server_password: server_password(&app),
            pid: Some(state.pid.load(Ordering::SeqCst)),
            workspace: state.workspace.lock().ok().and_then(|w| w.clone()),
            error: None,
        });
    }

    let native_dir =
        native_library_dir().ok_or_else(|| "native library directory not found".to_string())?;
    let runtime = node_runtime_path(&native_dir);
    if !runtime.exists() {
        return Err(format!("node runtime missing at {}", runtime.display()));
    }

    // Executable launch strategies, most preferred first.
    //
    // 1. exec the runtime straight from nativeLibraryDir — the package
    //    manager owns that directory, so the app cannot tamper with it and
    //    Android allows exec() there (this is how apps ship helper binaries).
    // 2. `/system/bin/linker64 <runtime>` — used when the platform refuses
    //    direct exec (the linker maps the ELF instead).
    let entry = server_entry(&app).ok_or_else(|| "bundle path unavailable".to_string())?;
    if !entry.exists() {
        return Err(format!(
            "opencode server bundle missing at {} (assets not installed yet)",
            entry.display()
        ));
    }

    let bundle_dir = bundle_root(&app).ok_or_else(|| "bundle path unavailable".to_string())?;
    let home = home_dir(&app).ok_or_else(|| "home path unavailable".to_string())?;

    // XDG roots inside the app sandbox: opencode writes config, sessions and
    // its SQLite database there.
    for dir in [
        home.clone(),
        home.join(".local").join("share"),
        home.join(".local").join("state"),
        home.join(".config"),
        home.join(".cache"),
        home.join("bin"),
        bundle_dir.join("tmp"),
    ] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    }

    // ripgrep ships as `librg.so` inside the native lib dir (that is the only
    // place an app may exec from). Expose it under the plain name opencode's
    // search tool looks for, through a symlink in the app's bin dir.
    let rg_target = native_dir.join(RIPGREP_NAME);
    if rg_target.exists() {
        let link = home.join("bin").join("rg");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&rg_target, &link);
    }

    let password = server_password(&app).ok_or_else(|| "password unavailable".to_string())?;

    // Workspace: the directory the agent operates in. Falls back to the
    // sandbox home so the server always starts with a valid cwd.
    let workspace_dir = workspace
        .as_ref()
        .map(|w| PathBuf::from(w))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| home.clone());
    if !workspace_dir.is_dir() {
        return Err(format!("workspace is not a directory: {}", workspace_dir.display()));
    }

    let mut cmd = build_server_command(
        Launcher::Direct,
        &runtime,
        &entry,
        &workspace_dir,
        &home,
        &bundle_dir,
        &native_dir,
        &password,
        env_vars.clone().unwrap_or_default(),
        extra_args.clone().unwrap_or_default(),
    );

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(first_error) => {
            // Fallback: run the runtime through the system dynamic linker,
            // which maps the ELF instead of exec()ing it directly.
            let abi = native_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let linker = if abi == "arm64" || abi == "x86_64" {
                PathBuf::from("/system/bin/linker64")
            } else {
                PathBuf::from("/system/bin/linker")
            };
            if !linker.exists() {
                return Err(format!("failed to spawn node: {}", first_error));
            }
            log::warn!("direct exec failed ({}), retrying through {}", first_error, linker.display());
            let mut via_linker = build_server_command(
                Launcher::Linker(linker.as_path()),
                &runtime,
                &entry,
                &workspace_dir,
                &home,
                &bundle_dir,
                &native_dir,
                &password,
                env_vars.clone().unwrap_or_default(),
                extra_args.clone().unwrap_or_default(),
            );
            via_linker.spawn().map_err(|e| {
                format!("failed to spawn node ({} / linker64: {})", first_error, e)
            })?
        }
    };
    let pid = child.id();

    // Drain stdout/stderr into a channel so the startup banner can be parsed.
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, tx.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, tx);
    }

    // Wait for the listening banner.
    let mut url: Option<String> = None;
    let mut detected_password: Option<String> = None;
    let mut recent: Vec<String> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);

    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                log::info!("[embedded] {}", line);
                recent.push(line.clone());
                if recent.len() > 20 {
                    recent.remove(0);
                }
                if url.is_none() {
                    url = parse_listening_url(&line);
                }
                if detected_password.is_none() {
                    detected_password = parse_server_password(&line);
                }
                if url.is_some() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!(
                        "server exited during startup ({}): {}",
                        status,
                        recent.join(" | ")
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!("server exited during startup ({}): {}", status, recent.join(" | ")));
                }
            }
        }
    }

    let url = match url {
        Some(url) => url,
        None => {
            let _ = child.kill();
            return Err(format!("server did not report a listening address: {}", recent.join(" | ")));
        }
    };

    // The server may have picked a different port than requested.
    let password = detected_password.unwrap_or(password);

    // Health check: the server answers /api/info only once it is ready.
    let ready = wait_for_ready(&url, &password, Duration::from_secs(30)).await;
    if !ready {
        let _ = child.kill();
        return Err("server did not become ready".to_string());
    }

    state.pid.store(pid, Ordering::SeqCst);
    *state.url.lock().map_err(|_| "state poisoned")? = Some(url.clone());
    *state.workspace.lock().map_err(|_| "state poisoned")? =
        Some(workspace_dir.to_string_lossy().to_string());
    if let Ok(mut slot) = state.child.lock() {
        *slot = Some(child);
    }

    Ok(EmbeddedServerResult {
        started: true,
        url: Some(url),
        server_password: Some(password),
        pid: Some(pid),
        workspace: Some(workspace_dir.to_string_lossy().to_string()),
        error: None,
    })
}

#[tauri::command]
pub fn stop_embedded_server(state: State<'_, EmbeddedServerState>) -> Result<bool, String> {
    let pid = state.pid.swap(0, Ordering::SeqCst);
    *state.url.lock().map_err(|_| "state poisoned")? = None;
    let mut killed = false;
    if let Ok(mut slot) = state.child.lock() {
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
        }
    }
    if !killed && pid > 0 {
        // The process was started by an earlier app session; kill by pid.
        let _ = Command::new("kill").arg(pid.to_string()).status();
        killed = true;
    }
    Ok(killed)
}

// ============================================
// Workspace browsing
//
// The agent works on real device storage. With MANAGE_EXTERNAL_STORAGE the
// app can read /sdcard directly, so the workspace is picked by browsing the
// filesystem instead of a SAF document tree (which would hand out content://
// URIs the server cannot use as a working directory).
// ============================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// True when the directory can be used as a workspace (writable).
    pub writable: bool,
}

#[tauri::command]
pub fn workspace_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    for path in ["/sdcard", "/storage/emulated/0", "/storage/self/primary"] {
        let path = PathBuf::from(path);
        if path.is_dir() {
            candidates.push(path.to_string_lossy().to_string());
        }
    }
    candidates
}

#[tauri::command]
pub fn list_workspace_dirs(path: String) -> Result<Vec<WorkspaceEntry>, String> {
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", path));
    }
    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    let read = std::fs::read_dir(&dir).map_err(|e| format!("read {}: {}", path, e))?;
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let full = entry.path();
        entries.push(WorkspaceEntry {
            name,
            path: full.to_string_lossy().to_string(),
            is_dir,
            writable: is_dir && !std::fs::metadata(&full).map(|m| m.permissions().readonly()).unwrap_or(true),
        });
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}


// ============================================
// Helpers
// ============================================

/// How the bundled runtime is executed.
enum Launcher<'a> {
    /// exec the runtime directly.
    Direct,
    /// run the runtime through the system dynamic linker.
    Linker(&'a Path),
}

/// Builds the `node opencode.mjs serve` command.
#[allow(clippy::too_many_arguments)]
fn build_server_command(
    launcher: Launcher<'_>,
    runtime: &Path,
    entry: &Path,
    workspace: &Path,
    home: &Path,
    bundle_dir: &Path,
    native_dir: &Path,
    password: &str,
    env_vars: HashMap<String, String>,
    extra_args: Vec<String>,
) -> Command {
    let mut cmd = match launcher {
        Launcher::Direct => Command::new(runtime),
        Launcher::Linker(linker) => {
            let mut cmd = Command::new(linker);
            cmd.arg(runtime);
            cmd
        }
    };

    cmd.arg(entry);
    cmd.arg("serve");
    cmd.arg("--hostname");
    cmd.arg("127.0.0.1");
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.current_dir(workspace);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Environment: XDG roots, the server password, and the tool PATH.
    cmd.env("HOME", home);
    cmd.env("TMPDIR", bundle_dir.join("tmp"));
    cmd.env("XDG_DATA_HOME", home.join(".local").join("share"));
    cmd.env("XDG_STATE_HOME", home.join(".local").join("state"));
    cmd.env("XDG_CONFIG_HOME", home.join(".config"));
    cmd.env("XDG_CACHE_HOME", home.join(".cache"));
    cmd.env("OPENCODE_PASSWORD", password);
    // No updater and no glibc-native helpers on Android. The parcel watcher
    // and fff bindings are also missing, and every consumer degrades
    // gracefully when they are absent.
    cmd.env("OPENCODE_DISABLE_AUTOUPDATE", "1");
    cmd.env("OPENCODE_FILEWATCHER_DISABLE", "1");
    cmd.env("OPENCODE_DISABLE_FFF", "1");
    // Android ships mksh in /system/bin, not bash; pin the shell so the
    // agent's shell tool does not look for /bin/bash.
    cmd.env("SHELL", "/system/bin/sh");
    cmd.env("TERM", "dumb");

    // PATH: the app's bin dir (ripgrep symlink), the native dir (runtime),
    // then the system shell utilities the agent's bash tool needs.
    let mut path_entries = vec![
        home.join("bin").to_string_lossy().to_string(),
        native_dir.to_string_lossy().to_string(),
        "/system/bin".to_string(),
        "/bin".to_string(),
    ];
    path_entries.dedup();
    cmd.env("PATH", path_entries.join(":"));

    // User supplied environment (API keys, proxy settings, ...).
    for (key, value) in env_vars {
        if key.trim().is_empty() {
            continue;
        }
        cmd.env(key, value);
    }

    cmd
}

fn spawn_reader<R: std::io::Read + Send + 'static>(reader: R, tx: mpsc::Sender<String>) {
    thread::spawn(move || {
        let reader = std::io::BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
}

async fn wait_for_ready(url: &str, password: &str, timeout: Duration) -> bool {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let request = client
            .get(format!("{}/api/info", url.trim_end_matches('/')))
            .basic_auth("opencode", Some(password));
        if let Ok(response) = request.timeout(Duration::from_secs(5)).send().await {
            if response.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    false
}
