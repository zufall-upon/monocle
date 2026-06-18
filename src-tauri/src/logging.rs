//! Lightweight always-on file logging for catching field bugs. Records the
//! events that have actually bitten us — activation triggers (with their
//! source), shake fires (with timing), and focus changes (with the computed
//! sharp group) — to %APPDATA%\Deep\deep.log. The file is rotated to
//! `.old` once when it grows past a couple of megabytes, so it never grows
//! unbounded but recent history is always available.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

/// Full path to the active log file (next to settings.json).
pub fn log_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Deep");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("deep.log")
}

/// Open the log for appending, rotating it first if it's gotten large. Call
/// once at startup.
pub fn init() {
    let path = log_path();
    if std::fs::metadata(&path)
        .map(|m| m.len() > MAX_LOG_BYTES)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, path.with_file_name("deep.log.old"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *LOG_FILE.lock().unwrap() = Some(file);
    }
    log(&format!("=== Deep v{} started ===", env!("CARGO_PKG_VERSION")));
}

/// Append one timestamped line. Flushes immediately so the log survives a
/// crash. A no-op (never panics) if the file couldn't be opened.
pub fn log(msg: &str) {
    let line = format!("{} {}\n", timestamp(), msg);
    if let Ok(mut guard) = LOG_FILE.lock() {
        if let Some(file) = guard.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

#[cfg(windows)]
fn timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    String::new()
}
