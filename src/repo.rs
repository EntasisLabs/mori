use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const REPO_DIR: &str = ".mori";
const CONFIG_FILE: &str = "config.json";
const INDEX_FILE: &str = "index.json";
const STASH_FILE: &str = "stash.json";
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

    pub fn stash_path(&self) -> PathBuf {
        self.mori_dir().join(STASH_FILE)
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

    pub fn save_config(&self, config: &RepoConfig) -> Result<()> {
        fs::write(
            self.config_path(),
            serde_json::to_string_pretty(config)? + "\n",
        )
        .with_context(|| format!("writing {}", self.config_path().display()))?;
        Ok(())
    }

    /// Stable id for this repo, used as the push checkpoint key on a remote.
    pub fn ensure_repo_id(&self) -> Result<String> {
        let mut config = self.load_config()?;
        if !config.repo_id.trim().is_empty() {
            return Ok(config.repo_id);
        }
        config.repo_id = Uuid::new_v4().simple().to_string();
        self.save_config(&config)?;
        Ok(config.repo_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub version: u32,
    pub default_session: String,
    pub created_at: DateTime<Utc>,
    /// Identifies this repo to a remote so two clones do not share a push cursor.
    #[serde(default)]
    pub repo_id: String,
    #[serde(default)]
    pub remotes: BTreeMap<String, RemoteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub endpoint: String,
    #[serde(default = "default_namespace")]
    pub namespace: String,
    #[serde(default = "default_database")]
    pub database: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

fn default_namespace() -> String {
    "mori".to_string()
}

fn default_database() -> String {
    "memory".to_string()
}

pub fn validate_remote_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.eq_ignore_ascii_case("head") {
        bail!("HEAD is reserved");
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("remote name cannot be empty");
    };
    let rest_ok = chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-');
    if name.len() > 64 || !first.is_ascii_alphanumeric() || !rest_ok {
        bail!("remote names use letters, digits, '.', '_' and '-'");
    }
    Ok(name)
}

pub fn validate_remote_endpoint(endpoint: &str) -> Result<&str> {
    let endpoint = endpoint.trim();
    let lower = endpoint.to_ascii_lowercase();
    let remote = lower.starts_with("ws://")
        || lower.starts_with("wss://")
        || lower.starts_with("http://")
        || lower.starts_with("https://");
    let embedded = lower.starts_with("surrealkv://");
    if !remote && !embedded {
        bail!("remote endpoint must be ws://, wss://, http://, or https://");
    }
    let scheme_len = endpoint.find("://").unwrap_or(0) + 3;
    if endpoint.len() <= scheme_len {
        bail!("remote endpoint is missing a host");
    }
    Ok(endpoint)
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
        repo_id: Uuid::new_v4().simple().to_string(),
        remotes: BTreeMap::new(),
    };
    fs::write(
        repo.config_path(),
        serde_json::to_string_pretty(&config)? + "\n",
    )?;
    fs::write(repo.index_path(), "{}\n")?;
    Ok(repo)
}
