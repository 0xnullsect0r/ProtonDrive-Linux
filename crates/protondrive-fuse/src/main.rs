//! `protondrive-fs` binary — mounts the FUSE filesystem.
//!
//! Usage: `protondrive-fs [MOUNT_POINT]`
//!
//! If `MOUNT_POINT` is omitted, falls back to the configured one (default
//! `/mnt/ProtonDrive`, with XDG_DATA_HOME fallback if that isn't writable).

mod fs;

use anyhow::Result;
use fuser::MountOption;
use protondrive_core::Daemon;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,protondrive=debug".into()))
        .init();

    let daemon = Daemon::init()?;
    let _bg = daemon.spawn_sync();

    let mount_point = std::env::args().nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| daemon.config.resolved_mount_point(&daemon.paths));
    std::fs::create_dir_all(&mount_point)?;

    tracing::info!(?mount_point, "mounting Proton Drive");

    let opts = vec![
        MountOption::FSName("protondrive".into()),
        MountOption::Subtype("protondrive".into()),
        MountOption::DefaultPermissions,
        MountOption::AutoUnmount,
    ];
    let fs = fs::ProtonFs::new(&daemon, None);
    fuser::mount2(fs, &mount_point, &opts)?;
    Ok(())
}
