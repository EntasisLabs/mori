use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const REPO_DIR: &str = ".mori";
const CONFIG_FILE: &str = "config.json";
const INDEX_FILE: &str = "index.json";
const KV_DIR: &str = "kv";

#[derive(Debug, Clone)]
pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub fn mori_dir(&self) -> PathBuf {
        self.root.join(REPO_DIR)
    }

    pub fn config_path(&self) -> PathBuf {
        self.mori_dir().join(CONFIG_FILE)
    }

    pub fn index_path(&self) -> PathBuf {
        self.mori_dir().join(INDEX_FILE)
    }

    /// Directory SurrealKV owns. The adapter opens `surrealkv://<this path>`.
    pub fn kv_dir(&self) -> PathBuf {
        self.mori_dir().join(KV_DIR)
    }

    pub fn load_config(&self) -> Result<RepoConfig> {
        let text = fs::read_to_string(self.config_path())
            .with_context(|| format!("reading {}", self.config_path().display()))?;
        Ok(serde_json::from_str(&text).context("parsing .mori/config.json")?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub version: u32,
    pub default_session: String,
    pub created_at: DateTime<Utc>,
}

/// Walk from `start` toward the filesystem root looking for `.mori/config.json`.
pub fn find_repo(start: &Path) -> Result<Repo> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent().unwrap_or(start).to_path_buf()
    };

    loop {
        let candidate = current.join(REPO_DIR).join(CONFIG_FILE);
        if candidate.is_file() {
            return Ok(Repo { root: current });
        }
        if !current.pop() {
            break;
        }
    }

    bail!("not a mori repository (or any parent). Run `mori init` first");
}

/// Create `.mori` with a config and an empty index. Does not open the database.
pub fn init_repo(path: &Path, default_session: &str) -> Result<Repo> {
    let root = if path.as_os_str().is_empty() {
        std::env::current_dir()?
    } else {
        path.to_path_buf()
    };
    fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    let root = root.canonicalize().unwrap_or(root);

    let repo = Repo { root: root.clone() };
    if repo.config_path().exists() {
        bail!(
            "mori repository already exists at {}",
            repo.mori_dir().display()
        );
    }

    let session = default_session.trim();
    if session.is_empty() {
        bail!("session name cannot be empty");
    }

    fs::create_dir_all(repo.mori_dir())?;
    let config = RepoConfig {
        version: 1,
        default_session: session.to_string(),
        created_at: Utc::now(),
    };
    fs::write(
        repo.config_path(),
        serde_json::to_string_pretty(&config)? + "\n",
    )?;
    fs::write(repo.index_path(), "{}\n")?;
    Ok(repo)
}
