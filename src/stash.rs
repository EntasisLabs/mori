use std::fs;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::repo::Repo;
use crate::stage::StagedContext;

/// One parked index. Newest entries sit at the front of the stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StashEntry {
    pub id: String,
    pub branch: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<StagedContext>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StashFile {
    #[serde(default)]
    entries: Vec<StashEntry>,
}

pub struct Stash {
    entries: Vec<StashEntry>,
}

impl Stash {
    pub fn load(repo: &Repo) -> Result<Self> {
        let path = repo.stash_path();
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
        let file: StashFile = serde_json::from_str(&text).context("parsing .mori/stash.json")?;
        Ok(Self {
            entries: file.entries,
        })
    }

    pub fn save(&self, repo: &Repo) -> Result<()> {
        let file = StashFile {
            entries: self.entries.clone(),
        };
        fs::write(
            repo.stash_path(),
            serde_json::to_string_pretty(&file)? + "\n",
        )?;
        Ok(())
    }

    pub fn entries(&self) -> &[StashEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push(&mut self, branch: &str, message: &str, staged: Vec<StagedContext>) -> &StashEntry {
        let message = {
            let trimmed = message.trim();
            if trimmed.is_empty() {
                default_message(&staged)
            } else {
                trimmed.to_string()
            }
        };
        self.entries.insert(
            0,
            StashEntry {
                id: Uuid::new_v4().simple().to_string(),
                branch: branch.to_string(),
                message,
                created_at: Utc::now(),
                entries: staged,
            },
        );
        &self.entries[0]
    }

    pub fn pop(&mut self) -> Result<StashEntry> {
        if self.entries.is_empty() {
            bail!("nothing stashed");
        }
        Ok(self.entries.remove(0))
    }
}

fn default_message(staged: &[StagedContext]) -> String {
    match staged {
        [] => "wip".to_string(),
        [one] => format!("wip {}", one.source),
        many => format!("wip {} (+{})", many[0].source, many.len() - 1),
    }
}
