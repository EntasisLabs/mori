use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use locus_core_rs::domain::contracts::{
    NodeStore, NodeStoreInitializer, SemanticIndexStore, SemanticIndexStoreInitializer,
};
use locus_core_rs::domain::models::AvecState;
use locus_core_rs::storage::surrealdb::QueryParams;
use locus_core_rs::{
    StoreContextService, SttpNodeParser, SurrealDbClient, SurrealDbNodeStore,
    SurrealDbRuntimeOptions, SurrealDbSemanticIndexStore, TreeSitterValidator,
};
use locus_sdk::prelude::{
    FallbackPolicy, MemoryFindRequest, MemoryFindService, MemoryPage, MemoryRecallRequest,
    MemoryRecallService, MemoryScope, MemoryScoring, StrictnessMode,
};
use locus_surreal_adapter::RuntimeSurrealDbClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::compile::compile_context;
use crate::repo::Repo;
use crate::stage::{ContextKind, StagedContext};

/// One embedded SurrealKV connection.
///
/// Construct it at the start of a command and drop it before the command
/// returns. There is no resident process.
pub struct Memory {
    client: Arc<RuntimeSurrealDbClient>,
    store: Arc<SurrealDbNodeStore>,
    index: Arc<SurrealDbSemanticIndexStore>,
    endpoint: String,
}

impl Memory {
    pub async fn connect(repo: &Repo) -> Result<Self> {
        let kv_dir = repo.kv_dir();
        std::fs::create_dir_all(&kv_dir)
            .with_context(|| format!("creating {}", kv_dir.display()))?;
        let endpoint = format!("surrealkv://{}", kv_dir.display());
        let runtime = SurrealDbRuntimeOptions {
            root_dir: repo.root.display().to_string(),
            use_remote: false,
            endpoint: endpoint.clone(),
            namespace: "mori".to_string(),
            database: "memory".to_string(),
        };

        let client = Arc::new(
            RuntimeSurrealDbClient::connect(&runtime, None, None)
                .await
                .with_context(|| format!("connecting to {endpoint}"))?,
        );
        let index = Arc::new(SurrealDbSemanticIndexStore::new(client.clone()));
        let store = Arc::new(SurrealDbNodeStore::new(client.clone()));

        let initializer: Arc<dyn NodeStoreInitializer> = store.clone();
        initializer.initialize_async().await?;
        let semantic_initializer: Arc<dyn SemanticIndexStoreInitializer> = index.clone();
        semantic_initializer.initialize_async().await?;

        client
            .raw_query(
                "DEFINE TABLE IF NOT EXISTS mori_commit SCHEMALESS;
                 DEFINE TABLE IF NOT EXISTS mori_ref SCHEMALESS;",
                QueryParams::new(),
            )
            .await
            .context("preparing mori commit tables")?;

        Ok(Self {
            client,
            store,
            index,
            endpoint,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Drop the embedded engine. Safe to call explicitly at the end of a command.
    pub fn disconnect(self) {}

    pub async fn commit(
        &self,
        message: &str,
        session: &str,
        staged: &[StagedContext],
    ) -> Result<CommitRecord> {
        let message = message.trim();
        if message.is_empty() {
            bail!("commit message cannot be empty");
        }
        if staged.is_empty() {
            bail!("nothing staged");
        }

        let store_context = StoreContextService::new(
            self.store.clone() as Arc<dyn NodeStore>,
            Arc::new(TreeSitterValidator::new()),
            SttpNodeParser::new(),
        )
        .with_semantic_index(self.index.clone() as Arc<dyn SemanticIndexStore>);

        let mut entries = Vec::with_capacity(staged.len());
        for item in staged {
            let compiled = compile_context(item)?;
            let stored = store_context
                .store_async(&compiled.raw_sttp, &compiled.session)
                .await;
            if !stored.valid {
                bail!(
                    "could not store {}: {}",
                    compiled.source,
                    stored
                        .validation_error
                        .unwrap_or_else(|| "store rejected the context".to_string())
                );
            }
            entries.push(CommitEntry {
                node_id: stored.node_id,
                kind: compiled.kind,
                session: compiled.session,
                source: compiled.source,
                summary: compiled.summary,
            });
        }

        let parent = self.head_id().await?;
        let record = CommitRecord {
            id: Uuid::new_v4().simple().to_string(),
            parent,
            message: message.to_string(),
            session: session.to_string(),
            created_at: Utc::now(),
            entries,
        };

        let mut params = QueryParams::new();
        params.insert("mori_commit_id".to_string(), json!(record.id));
        params.insert("mori_parent".to_string(), json!(record.parent));
        params.insert("mori_message".to_string(), json!(record.message));
        params.insert("mori_scope".to_string(), json!(record.session));
        params.insert(
            "mori_created_at".to_string(),
            json!(record.created_at.to_rfc3339()),
        );
        params.insert(
            "mori_entries".to_string(),
            serde_json::to_value(&record.entries)?,
        );

        self.client
            .raw_query(
                &format!(
                    "CREATE mori_commit:`{}` SET commit_id = $mori_commit_id, parent = $mori_parent, message = $mori_message, session = $mori_scope, created_at = $mori_created_at, entries = $mori_entries;",
                    record.id
                ),
                params,
            )
            .await
            .context("writing commit")?;

        let mut head_params = QueryParams::new();
        head_params.insert("mori_commit_id".to_string(), json!(record.id));
        self.client
            .raw_query(
                "UPSERT mori_ref:HEAD SET commit_id = $mori_commit_id;",
                head_params,
            )
            .await
            .context("updating HEAD")?;

        Ok(record)
    }

    pub async fn head(&self) -> Result<Option<CommitRecord>> {
        let Some(id) = self.head_id().await? else {
            return Ok(None);
        };
        self.commit_by_id(&id).await
    }

    pub async fn log(&self, limit: usize) -> Result<Vec<CommitRecord>> {
        let Some(head) = self.head_id().await? else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        let mut cursor = Some(head);
        let limit = limit.max(1);
        while let Some(id) = cursor {
            if records.len() >= limit {
                break;
            }
            let Some(record) = self.commit_by_id(&id).await? else {
                break;
            };
            cursor = record.parent.clone();
            records.push(record);
        }
        Ok(records)
    }

    pub async fn find_commit(&self, prefix: &str) -> Result<CommitRecord> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            bail!("commit id is empty");
        }
        let rows = self
            .client
            .raw_query("SELECT * FROM mori_commit;", QueryParams::new())
            .await
            .context("listing commits")?;
        let matches = rows
            .into_iter()
            .filter_map(|row| commit_from_row(&row).ok())
            .filter(|record| record.id.starts_with(prefix))
            .collect::<Vec<_>>();
        match matches.len() {
            0 => bail!("no commit matches '{prefix}'"),
            1 => Ok(matches.into_iter().next().unwrap()),
            _ => bail!("commit prefix '{prefix}' is ambiguous"),
        }
    }

    pub async fn node_raw(&self, node_id: &str) -> Result<String> {
        let id = node_id
            .trim()
            .trim_start_matches("temporal_node:")
            .replace('`', "");
        if id.is_empty() || id.contains(';') {
            bail!("invalid node id");
        }
        let rows = self
            .client
            .raw_query(
                &format!("SELECT raw FROM temporal_node:`{id}`;"),
                QueryParams::new(),
            )
            .await
            .context("reading stored node")?;
        rows.first()
            .and_then(|row| row.get("raw"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("node {id} has no stored text"))
    }

    pub async fn recall(
        &self,
        query: &str,
        session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Recalled>> {
        let recall = MemoryRecallService::new(self.store.clone() as Arc<dyn NodeStore>)
            .with_semantic_index(self.index.clone() as Arc<dyn SemanticIndexStore>);
        let result = recall
            .execute(&MemoryRecallRequest {
                scope: scope_for(session),
                page: MemoryPage {
                    limit: limit.max(1),
                    cursor: None,
                },
                scoring: MemoryScoring {
                    fallback_policy: FallbackPolicy::OnEmpty,
                    strictness: StrictnessMode::Balanced,
                    lexical_weight: 0.4,
                    ..MemoryScoring::default()
                },
                current_avec: Some(AvecState::analytical()),
                query_text: Some(query.to_string()),
                ..MemoryRecallRequest::default()
            })
            .await?;

        let path = format!("{:?}", result.retrieval_path).to_ascii_lowercase();
        Ok(result
            .nodes
            .into_iter()
            .map(|node| Recalled {
                session: node.session_id,
                summary: node
                    .context_summary
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "(stored context)".to_string()),
                raw_sttp: node.raw,
                path: path.clone(),
            })
            .collect())
    }

    pub async fn find(
        &self,
        session: Option<&str>,
        contains: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Recalled>> {
        let find = MemoryFindService::new(self.store.clone() as Arc<dyn NodeStore>)
            .with_semantic_index(self.index.clone() as Arc<dyn SemanticIndexStore>);
        let mut request = MemoryFindRequest {
            scope: scope_for(session),
            page: MemoryPage {
                limit: limit.max(1),
                cursor: None,
            },
            ..MemoryFindRequest::default()
        };
        if let Some(text) = contains.map(str::trim).filter(|text| !text.is_empty()) {
            request.filter.text_contains = Some(text.to_string());
        }
        let result = find.execute(&request).await?;
        Ok(result
            .nodes
            .into_iter()
            .map(|node| Recalled {
                session: node.session_id,
                summary: node
                    .context_summary
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| "(stored context)".to_string()),
                raw_sttp: node.raw,
                path: "find".to_string(),
            })
            .collect())
    }

    async fn head_id(&self) -> Result<Option<String>> {
        let rows = self
            .client
            .raw_query("SELECT * FROM mori_ref:HEAD;", QueryParams::new())
            .await
            .context("reading HEAD")?;
        Ok(rows
            .first()
            .and_then(|row| row.get("commit_id"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    async fn commit_by_id(&self, id: &str) -> Result<Option<CommitRecord>> {
        let rows = self
            .client
            .raw_query(
                &format!("SELECT * FROM mori_commit:`{id}`;"),
                QueryParams::new(),
            )
            .await
            .context("reading commit")?;
        match rows.first() {
            Some(row) => Ok(Some(commit_from_row(row)?)),
            None => Ok(None),
        }
    }
}

fn scope_for(session: Option<&str>) -> MemoryScope {
    MemoryScope {
        session_ids: session
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()]),
        ..MemoryScope::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitEntry {
    pub node_id: String,
    pub kind: ContextKind,
    pub session: String,
    pub source: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub id: String,
    pub parent: Option<String>,
    pub message: String,
    pub session: String,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<CommitEntry>,
}

#[derive(Debug, Clone)]
pub struct Recalled {
    pub session: String,
    pub summary: String,
    pub raw_sttp: String,
    pub path: String,
}

fn commit_from_row(row: &Value) -> Result<CommitRecord> {
    let id = row
        .get("commit_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("commit row is missing commit_id"))?;
    let created_at = row
        .get("created_at")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let entries = match row.get("entries") {
        Some(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
        None => Vec::new(),
    };
    let parent = row.get("parent").and_then(|value| match value {
        Value::String(text) if !text.is_empty() && text != "null" => Some(text.clone()),
        _ => None,
    });
    Ok(CommitRecord {
        id,
        parent,
        message: row
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        session: row
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string(),
        created_at,
        entries,
    })
}
