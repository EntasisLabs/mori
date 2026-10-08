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
    /// User facets. Stored alongside the automatic `document` and `source:` tags.
    #[serde(default)]
    pub tags: Vec<String>,
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

    pub fn replace(&mut self, entries: Vec<StagedContext>) {
        self.entries = entries;
    }

    pub fn unstage(&mut self, source: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.source != source);
        self.entries.len() != before
    }
}

/// Normalize `--tag` values.
///
/// Repeating `--tag` and a comma list (`--tag homelab,pxe`) both work. Tags are
/// trimmed, lowercased, and de-duplicated so they match Locus semantic tags.
pub fn normalize_tags(values: &[String]) -> Result<Vec<String>> {
    let mut tags = Vec::new();
    for value in values {
        for part in value.split(',') {
            let tag = part.trim().to_lowercase();
            if tag.is_empty() {
                continue;
            }
            if !valid_tag(&tag) {
                bail!(
                    "tag '{tag}' is invalid; use at most 64 characters and no quotes, backslashes, or structural markers"
                );
            }
            if !tags.iter().any(|existing| existing == &tag) {
                tags.push(tag);
            }
        }
    }
    Ok(tags)
}

fn valid_tag(tag: &str) -> bool {
    tag.len() <= 64
        && !tag.contains("ref:")
        && !tag
            .chars()
            .any(|ch| matches!(ch, '"' | '\\' | '\n' | '\r' | '⊕' | '⦿' | '◈' | '⍉'))
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

#[cfg(test)]
mod tests {
    use super::normalize_tags;

    #[test]
    fn tags_split_on_commas_and_ignore_case() {
        let tags = normalize_tags(&[
            "HomeLab,pxe".to_string(),
            "jellyfin".to_string(),
            "pxe".to_string(),
        ])
        .unwrap();
        assert_eq!(tags, vec!["homelab", "pxe", "jellyfin"]);
    }

    #[test]
    fn tags_reject_structural_markers() {
        let err = normalize_tags(&["bad⊕tag".to_string()]).unwrap_err();
        assert!(err.to_string().contains("invalid"), "{err}");
    }
}
