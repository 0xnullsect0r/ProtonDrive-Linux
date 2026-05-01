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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,protondrive=debug".into()),
        )
        .init();

    // Tokio runtime for the async daemon, spawned on a dedicated thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let daemon = Daemon::init()?;
    let sync_ctrl = SyncController::new(rt.handle().clone());

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
