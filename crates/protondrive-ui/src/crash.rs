//! Opt-in crash reporter.
//!
//! On startup, installs a panic hook that writes a crash report to
//! `$XDG_STATE_HOME/protondrive/crashes/<timestamp>.toml`.
//!
//! On next launch, if crash files exist and the user has opted in
//! (config flag), a GTK4 dialog is shown offering to send the report
//! to the configured endpoint.  The endpoint URL is intentionally
//! empty by default; packagers or users can set it in `config.toml`.

use std::path::PathBuf;

/// Install the global panic hook.  Call once from `main` before any
/// other threads are spawned.
pub fn install_hook() {
    let crashes_dir = crash_dir();
    let _ = std::fs::create_dir_all(&crashes_dir);

    std::panic::set_hook(Box::new(move |info| {
        let now = chrono::Utc::now();
        let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let file = crashes_dir.join(format!("{stamp}.txt"));

        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());

        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                info.payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
            })
            .unwrap_or("<non-string payload>");

        let report = format!(
            "ProtonDrive crash report\ntime: {now}\nlocation: {location}\nmessage: {payload}\n\
             version: {}\n",
            env!("CARGO_PKG_VERSION")
        );
        let _ = std::fs::write(&file, report);

        // Also print to stderr so the original panic message isn't lost.
        eprintln!("PANIC at {location}: {payload}");
    }));
}

/// Returns the directory where crash reports are written.
pub fn crash_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local/state")
        })
        .join("protondrive")
        .join("crashes")
}

/// Returns the list of pending crash-report files (oldest first).
pub fn pending_reports() -> Vec<PathBuf> {
    let dir = crash_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "txt").unwrap_or(false))
        .collect();
    paths.sort();
    paths
}

/// Delete all pending crash reports (called after user dismisses or sends).
pub fn clear_reports() {
    for p in pending_reports() {
        let _ = std::fs::remove_file(p);
    }
}
