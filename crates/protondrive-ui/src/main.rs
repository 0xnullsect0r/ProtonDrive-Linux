//! `protondrive` binary — GTK4 + libadwaita UI + system tray.

mod sync;
mod tray;
mod ui;

use anyhow::Result;
use gtk4::prelude::*;
use libadwaita as adw;
use protondrive_core::Daemon;
use sync::SyncController;

const APP_ID: &str = "me.proton.drive.Linux";

fn main() -> Result<()> {
    // ── Structured logging ─────────────────────────────────────────
    // Write logs to $XDG_STATE_HOME/protondrive/log/ with daily rotation,
    // keeping them out of systemd journal and making "Copy Diagnostics" easy.
    let log_dir = dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(".local/state")
        })
        .join("protondrive")
        .join("log");
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "protondrive.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Also keep stderr output when RUST_LOG is set.
    use tracing_subscriber::prelude::*;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,protondrive=debug".into());
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr),
        )
        .with(env_filter)
        .init();

    // Tokio runtime for the async daemon, spawned on a dedicated thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let daemon = Daemon::init()?;
    let sync_ctrl = SyncController::new(rt.handle().clone());

    // Apply folder icons for file-manager integration on startup.
    {
        let sync_root = daemon.config.lock().sync_root.clone();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&sync_root);
            ui::apply_folder_icons(&sync_root);
        });
    }

    {
        let d = daemon.clone();
        let sc = sync_ctrl.clone();
        rt.spawn(async move {
            match d.try_resume().await {
                Ok(true) => {
                    tracing::info!("resumed previous Proton session");
                    if let Err(e) = sc.start(&d) {
                        tracing::warn!(error=%e, "auto-sync start failed after resume");
                    }
                }
                Ok(false) => tracing::info!("no stored session; awaiting sign-in"),
                Err(e) => tracing::warn!(error=%e, "session resume failed"),
            }
        });
    }

    // System tray on its own thread.
    let tray_daemon = daemon.clone();
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            if let Err(e) = tray::run(tray_daemon) {
                tracing::warn!(error=%e, "tray thread exited");
            }
        })?;

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        ui::build_main_window(app, daemon.clone(), sync_ctrl.clone())
    });
    app.run();
    Ok(())
}
