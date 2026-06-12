//! Verbose file logging + log export.
//!
//! Installs a `tracing` subscriber that writes verbose logs to a rolling file in
//! the OS app-log directory. Because the embedded server (`accord-server`) and
//! the mesh layer (`yggdrasil`/`ironwood`) all log via `tracing`, this single
//! subscriber captures **everything** in one place - ideal for troubleshooting.
//!
//! The dev menu/panel can open this folder so logs can be downloaded/shared.

use std::path::{Path, PathBuf};

use tauri::{App, AppHandle, Manager};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// The directory where logs are written.
#[must_use]
pub fn log_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_log_dir().ok()
}

/// Initialize logging. Returns a [`WorkerGuard`] that MUST be kept alive for the
/// process lifetime (it flushes the non-blocking writer); the caller leaks it.
///
/// Honors `RUST_LOG`; otherwise defaults to verbose for our crates + the mesh.
#[must_use]
pub fn init(app: &App) -> Option<WorkerGuard> {
    let dir = app.path().app_log_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;

    let file_appender = tracing_appender::rolling::daily(&dir, "accord.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,accord_client=debug,accord_server=debug,yggdrasil=debug,ironwood=debug",
        )
    });

    let initialized = tracing_subscriber::registry()
        .with(filter)
        // File layer (no ANSI colors in the file).
        .with(fmt::layer().with_ansi(false).with_writer(non_blocking))
        // Console layer for `cargo run` visibility.
        .with(fmt::layer().with_writer(std::io::stdout))
        .try_init()
        .is_ok();

    if initialized {
        tracing::info!(dir = %dir.display(), "verbose logging initialized");
        Some(guard)
    } else {
        None
    }
}

/// Open `path` in the platform file manager.
fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Reveal the logs folder in the file manager; returns the path. Used by both
/// the dev command and the dev menu.
pub fn open_logs(app: &AppHandle) -> Result<String, String> {
    let dir = log_dir(app).ok_or("no log directory available")?;
    open_path(&dir)?;
    Ok(dir.display().to_string())
}

/// Dev command: reveal the logs folder so logs can be downloaded/shared.
#[cfg(debug_assertions)]
#[tauri::command]
pub fn dev_open_logs(app: AppHandle) -> Result<String, String> {
    open_logs(&app)
}

/// Dev command: return the logs directory path (for display in the dev panel).
#[cfg(debug_assertions)]
#[tauri::command]
pub fn dev_log_dir(app: AppHandle) -> Result<String, String> {
    log_dir(&app)
        .map(|d| d.display().to_string())
        .ok_or_else(|| "no log directory".to_owned())
}
