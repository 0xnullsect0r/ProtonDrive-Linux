//! On-disk configuration and standard paths.
//!
//! Config lives at `$XDG_CONFIG_HOME/protondrive/config.toml`.
//! Cache lives at `$XDG_CACHE_HOME/protondrive/`.
//! Data (metadata DB) lives at `$XDG_DATA_HOME/protondrive/`.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

const QUALIFIER: &str = "me";
const ORG: &str = "proton";
const APP: &str = "protondrive";

/// User-visible configuration. Persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// The folder synced bidirectionally with Proton Drive.
    pub sync_root: PathBuf,
    /// Maximum total size of the on-disk block cache, in bytes.
    pub cache_max_bytes: u64,
    /// Polling interval for the sync loop, in seconds.
    pub poll_interval_secs: u64,
    /// Email for the Proton account (the password & TOTP secret live in the keyring).
    pub email: Option<String>,
    /// Top-level remote folder names to skip during sync (selective sync).
    #[serde(default)]
    pub excluded_paths: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let home = directories::UserDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            sync_root: home.join("ProtonDrive"),
            cache_max_bytes: 5 * 1024 * 1024 * 1024, // 5 GiB
            poll_interval_secs: 30,
            email: None,
            excluded_paths: Vec::new(),
        }
    }
}

/// Resolved on-disk paths derived from XDG dirs.
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let pd = ProjectDirs::from(QUALIFIER, ORG, APP)
            .ok_or_else(|| Error::Config("could not resolve XDG project dirs".into()))?;
        Ok(Self {
            config_dir: pd.config_dir().to_path_buf(),
            data_dir: pd.data_dir().to_path_buf(),
            cache_dir: pd.cache_dir().to_path_buf(),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn metadata_db(&self) -> PathBuf {
        self.data_dir.join("metadata.sqlite")
    }
    pub fn block_cache(&self) -> PathBuf {
        self.cache_dir.join("blocks")
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [
            &self.config_dir,
            &self.data_dir,
            &self.cache_dir,
            &self.block_cache(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

impl Config {
    /// Load config from disk, or return defaults if no file exists yet.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// If `sync_root` is not writable, fall back to
    /// `$XDG_DATA_HOME/ProtonDrive` and return the new path.
    pub fn resolved_sync_root(&self, paths: &Paths) -> PathBuf {
        if can_write(&self.sync_root) {
            self.sync_root.clone()
        } else {
            let fallback = paths.data_dir.join("ProtonDrive");
            tracing::warn!(
                "configured sync root {:?} is not writable; falling back to {:?}",
                self.sync_root,
                fallback
            );
            fallback
        }
    }
}

fn can_write(p: &Path) -> bool {
    if !p.exists() {
        // Try to create it (and immediately remove the marker).
        if std::fs::create_dir_all(p).is_err() {
            return false;
        }
    }
    let probe = p.join(".protondrive-writable");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}
