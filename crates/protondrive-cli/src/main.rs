//! Tiny headless CLI. Mostly for debugging / scripting.

use anyhow::Result;
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
            let cfg = daemon.config.lock();
            println!("sync_root     : {}", cfg.sync_root.display());
            println!("cache_max     : {} bytes", cfg.cache_max_bytes);
            println!("poll_interval : {}s", cfg.poll_interval_secs);
            println!(
                "email         : {}",
                cfg.email.as_deref().unwrap_or("(unset)")
            );
        }
        Some("resume") => match daemon.try_resume().await {
            Ok(true) => println!("resumed session"),
            Ok(false) => println!("no stored session"),
            Err(e) => {
                eprintln!("resume failed: {e}");
                std::process::exit(1);
            }
        },
        _ => {
            eprintln!("usage: protondrive-cli <status|resume>");
            std::process::exit(2);
        }
    }
    Ok(())
}
