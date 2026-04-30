//! Tiny headless CLI. Mostly for debugging / scripting.
//!
//! Subcommands:
//!   protondrive-cli status          — print current daemon state
//!   protondrive-cli refresh         — trigger a sync poll
//!   protondrive-cli pin    <NodeId> — mark a node "always available offline"
//!   protondrive-cli unpin  <NodeId>

use anyhow::Result;
use protondrive_core::types::NodeId;
use protondrive_core::Daemon;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,protondrive=debug".into()),
        )
        .init();

    let daemon = Daemon::init()?;
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("status") => {
            println!("mount_point   : {}", daemon.config.mount_point.display());
            println!("cache_max     : {} bytes", daemon.config.cache_max_bytes);
            println!("poll_interval : {}s", daemon.config.poll_interval_secs);
            println!(
                "email         : {}",
                daemon.config.email.as_deref().unwrap_or("(unset)")
            );
        }
        Some("refresh") => {
            let h = daemon.spawn_sync();
            daemon.sync.refresh_now();
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            h.abort();
            println!("refresh requested");
        }
        Some("pin") => {
            let id = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing NodeId"))?;
            daemon.db.set_pinned(&NodeId(id), true)?;
            println!("pinned");
        }
        Some("unpin") => {
            let id = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing NodeId"))?;
            daemon.db.set_pinned(&NodeId(id), false)?;
            println!("unpinned");
        }
        _ => {
            eprintln!("usage: protondrive-cli <status|refresh|pin <id>|unpin <id>>");
            std::process::exit(2);
        }
    }
    Ok(())
}
