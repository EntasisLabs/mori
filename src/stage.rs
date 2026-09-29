use std::fs;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::repo::Repo;

/// What the user handed us. STTP is one option, not the required shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Auto,
    Sttp,
    Document,
    Chat,
    Note,
    Context,
}

impl ContextKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "sttp" => Ok(Self::Sttp),
            "document" | "doc" => Ok(Self::Document),
            "chat" => Ok(Self::Chat),
            "note" => Ok(Self::Note),
            "context" => Ok(Self::Context),
            other => bail!(
                "unknown context kind '{other}'. Use auto, sttp, document, chat, note, or context"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Sttp => "sttp",
            Self::Document => "document",
            Self::Chat => "chat",
            Self::Note => "note",
            Self::Context => "context",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedContext {
    pub id: String,
    pub kind: ContextKind,
    pub session: String,
    /// Path the user named, or `-` when the text came from stdin.
    pub source: String,
    pub text: String,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    entries: Vec<StagedContext>,
}

pub struct StageIndex {
    entries: Vec<StagedContext>,
}

impl StageIndex {
    pub fn load(repo: &Repo) -> Result<Self> {
        let path = repo.index_path();
        if !path.exists() {
            return Ok(Self {
                entries: Vec::new(),
            });
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(Self {
                entries: Vec::new(),
            });
        }
        let file: IndexFile = serde_json::from_str(&text).context("parsing .mori/index.json")?;
        Ok(Self {
            entries: file.entries,
        })
    }

    pub fn save(&self, repo: &Repo) -> Result<()> {
        let file = IndexFile {
            entries: self.entries.clone(),
        };
        fs::write(
            repo.index_path(),
            serde_json::to_string_pretty(&file)? + "\n",
        )?;
        Ok(())
    }

    pub fn entries(&self) -> &[StagedContext] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Stage `source`. A later add of the same source path replaces the earlier one.
    pub fn upsert(&mut self, entry: StagedContext) {
        if entry.source != "-" {
            self.entries
                .retain(|existing| existing.source != entry.source);
        }
        self.entries.push(entry);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn unstage(&mut self, source: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.source != source);
        self.entries.len() != before
    }
}

pub fn read_source(source: &str) -> Result<String> {
    if source == "-" {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        return Ok(buffer);
    }

    let path = Path::new(source);
    if !path.is_file() {
        bail!("{source} is not a file");
    }
    fs::read_to_string(path).with_context(|| format!("reading {source}"))
}
