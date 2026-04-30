//! Top-level orchestrator. The UI and FUSE crates talk to this.

use std::sync::Arc;
use std::time::Duration;

use crate::api::ApiClient;
use crate::cache::{BlobCache, MetadataDb};
use crate::config::{Config, Paths};
use crate::sync::SyncEngine;
use crate::Result;

/// Bundles everything the rest of the app needs.
#[derive(Clone)]
pub struct Daemon {
    pub config: Config,
    pub paths: Arc<Paths>,
    pub api: ApiClient,
    pub db: MetadataDb,
    pub blobs: Arc<BlobCache>,
    pub sync: SyncEngine,
}

impl Daemon {
    /// Initialise everything from disk. Does **not** start the sync loop —
    /// call [`Self::spawn_sync`] for that.
    pub fn init() -> Result<Self> {
        let paths = Paths::discover()?;
        paths.ensure()?;
        let config = Config::load_or_default(&paths.config_file())?;

        let api = ApiClient::new()?;
        let db = MetadataDb::open(&paths.metadata_db())?;
        let blobs = Arc::new(BlobCache::new(paths.block_cache(), config.cache_max_bytes)?);

        let poll = Duration::from_secs(config.poll_interval_secs.max(5));
        let sync = SyncEngine::new(api.clone(), db.clone(), blobs.clone(), poll);

        Ok(Self {
            config,
            paths: Arc::new(paths),
            api,
            db,
            blobs,
            sync,
        })
    }

    /// Spawn the background sync loop on the current tokio runtime.
    pub fn spawn_sync(&self) -> tokio::task::JoinHandle<()> {
        let s = self.sync.clone();
        tokio::spawn(async move { s.run().await })
    }

    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.paths.config_file())
    }
}
