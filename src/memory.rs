use std::collections::{HashMap, HashSet};
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
use locus_sdk::application::memory_lexical::parse_lexical_query;
use locus_sdk::prelude::{
    FallbackPolicy, MemoryFilter, MemoryFindRequest, MemoryFindService, MemoryPage,
    MemoryRecallRequest, MemoryRecallService, MemoryScope, MemoryScoring, StrictnessMode,
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
                 DEFINE TABLE IF NOT EXISTS mori_ref SCHEMALESS;
                 DEFINE TABLE IF NOT EXISTS mori_branch SCHEMALESS;",
                QueryParams::new(),
            )
            .await
            .context("preparing mori commit tables")?;

        let memory = Self {
            client,
            store,
            index,
            endpoint,
        };
        memory.ensure_branch().await?;
        Ok(memory)
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
                tags: item.tags.clone(),
            });
        }

        let branch = self.current_branch().await?;
        let parent = self.branch_tip(&branch).await?;
        let record = CommitRecord {
            id: Uuid::new_v4().simple().to_string(),
            parent,
            merge_parents: Vec::new(),
            message: message.to_string(),
            session: session.to_string(),
            created_at: Utc::now(),
            entries,
        };
        self.store_commit(&record).await?;
        self.write_branch(&branch, Some(&record.id)).await?;
        Ok(record)
    }

    /// Bring `branch` into the current branch.
    ///
    /// A fast-forward moves the current pointer. A real merge writes one commit
    /// with both parents and no new context. Stored nodes stay where they are.
    pub async fn merge(
        &self,
        branch: &str,
        message: Option<&str>,
        session: &str,
    ) -> Result<MergeOutcome> {
        let branch = validate_branch_name(branch)?;
        let current = self.current_branch().await?;
        if branch == current {
            bail!("cannot merge a branch into itself");
        }
        let ours = self.branch_tip(&current).await?;
        let theirs = self.branch_tip(branch).await?;
        if self.is_ancestor(theirs.as_deref(), ours.as_deref()).await? {
            return Ok(MergeOutcome::UpToDate);
        }
        if self.is_ancestor(ours.as_deref(), theirs.as_deref()).await? {
            let commit_id = theirs
                .clone()
                .ok_or_else(|| anyhow!("branch '{branch}' has no commits"))?;
            self.write_branch(&current, Some(&commit_id)).await?;
            return Ok(MergeOutcome::FastForward { commit_id });
        }

        let message = message
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("merge {branch} into {current}"));
        let record = CommitRecord {
            id: Uuid::new_v4().simple().to_string(),
            parent: ours,
            merge_parents: vec![theirs.ok_or_else(|| anyhow!("branch '{branch}' has no commits"))?],
            message,
            session: session.to_string(),
            created_at: Utc::now(),
            entries: Vec::new(),
        };
        self.store_commit(&record).await?;
        self.write_branch(&current, Some(&record.id)).await?;
        Ok(MergeOutcome::Merged { commit: record })
    }

    /// Replay this branch's own commits onto `onto`.
    ///
    /// The target branch stays where it is. Merge commits are refused before
    /// anything is written, because replaying them would drop a parent.
    pub async fn rebase(&self, onto: &str) -> Result<RebaseOutcome> {
        let onto = validate_branch_name(onto)?;
        let current = self.current_branch().await?;
        if onto == current {
            bail!("cannot rebase a branch onto itself");
        }
        let onto_tip = self.branch_tip(onto).await?;
        let current_tip = self.branch_tip(&current).await?;
        let onto_history = self.reachable(onto_tip.as_deref()).await?;
        let pending = self
            .first_parent_commits(current_tip.as_deref(), &onto_history)
            .await?;
        if pending
            .iter()
            .any(|record| !record.merge_parents.is_empty())
        {
            bail!("rebase does not replay merge commits");
        }
        if pending.is_empty() {
            if current_tip == onto_tip {
                return Ok(RebaseOutcome::UpToDate);
            }
            let commit_id = onto_tip
                .clone()
                .ok_or_else(|| anyhow!("branch '{onto}' has no commits"))?;
            self.write_branch(&current, Some(&commit_id)).await?;
            return Ok(RebaseOutcome::FastForward { commit_id });
        }

        let mut parent = onto_tip;
        let mut commits = Vec::with_capacity(pending.len());
        for record in pending.into_iter().rev() {
            let replay = CommitRecord {
                id: Uuid::new_v4().simple().to_string(),
                parent: parent.clone(),
                merge_parents: Vec::new(),
                message: record.message,
                session: record.session,
                created_at: record.created_at,
                entries: record.entries,
            };
            self.store_commit(&replay).await?;
            parent = Some(replay.id.clone());
            commits.push(replay);
        }
        let tip = parent.ok_or_else(|| anyhow!("rebase produced no commits"))?;
        self.write_branch(&current, Some(&tip)).await?;
        Ok(RebaseOutcome::Rebased { commits, tip })
    }

    pub async fn current_branch(&self) -> Result<String> {
        let row = self
            .read_head()
            .await?
            .ok_or_else(|| anyhow!("HEAD is missing"))?;
        row.get("branch")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("HEAD does not name a branch"))
    }

    pub async fn branches(&self) -> Result<Vec<BranchInfo>> {
        let current = self.current_branch().await?;
        let rows = self
            .client
            .raw_query("SELECT * FROM mori_branch;", QueryParams::new())
            .await
            .context("listing branches")?;
        let mut branches = rows
            .iter()
            .filter_map(|row| branch_from_row(row))
            .map(|row| BranchInfo {
                current: row.name == current,
                name: row.name,
                commit_id: row.commit_id,
            })
            .collect::<Vec<_>>();
        branches.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(branches)
    }

    /// Point a new branch at the current tip. The current branch stays checked out.
    pub async fn create_branch(&self, name: &str) -> Result<String> {
        let name = validate_branch_name(name)?;
        if self.read_branch(name).await?.is_some() {
            bail!("branch '{name}' already exists");
        }
        let tip = self.branch_tip(&self.current_branch().await?).await?;
        self.write_branch(name, tip.as_deref()).await?;
        Ok(name.to_string())
    }

    pub async fn checkout_branch(&self, name: &str) -> Result<String> {
        let name = validate_branch_name(name)?;
        if self.read_branch(name).await?.is_none() {
            bail!("branch '{name}' does not exist");
        }
        self.set_head_branch(name).await?;
        Ok(name.to_string())
    }

    pub async fn branch_tip(&self, name: &str) -> Result<Option<String>> {
        let name = validate_branch_name(name)?;
        let Some(row) = self.read_branch(name).await? else {
            bail!("branch '{name}' does not exist");
        };
        Ok(row.commit_id)
    }

    pub async fn head(&self) -> Result<Option<CommitRecord>> {
        let Some(id) = self.branch_tip(&self.current_branch().await?).await? else {
            return Ok(None);
        };
        self.commit_by_id(&id).await
    }

    pub async fn log(&self, limit: usize) -> Result<Vec<CommitRecord>> {
        let head = self.branch_tip(&self.current_branch().await?).await?;
        self.walk_history(head.as_deref(), Some(limit.max(1))).await
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
        let id = sanitize_node_id(node_id)?;
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
        tags: &[String],
        limit: usize,
        widen: bool,
    ) -> Result<Vec<Recalled>> {
        let visible = self.visible_nodes().await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.max(1);
        let recall = MemoryRecallService::new(self.store.clone() as Arc<dyn NodeStore>)
            .with_semantic_index(self.index.clone() as Arc<dyn SemanticIndexStore>);
        let result = recall
            .execute(&MemoryRecallRequest {
                scope: scope_for(session),
                page: MemoryPage {
                    limit: scan_limit(limit),
                    cursor: None,
                },
                filter: memory_filter(tags, single_content_term(query)),
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
        Ok(take_visible(result.nodes, &visible, &path, limit, widen))
    }

    pub async fn find(
        &self,
        session: Option<&str>,
        contains: Option<&str>,
        tags: &[String],
        limit: usize,
        widen: bool,
    ) -> Result<Vec<Recalled>> {
        let visible = self.visible_nodes().await?;
        if visible.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.max(1);
        let find = MemoryFindService::new(self.store.clone() as Arc<dyn NodeStore>)
            .with_semantic_index(self.index.clone() as Arc<dyn SemanticIndexStore>);
        let text_contains = contains
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let request = MemoryFindRequest {
            scope: scope_for(session),
            page: MemoryPage {
                limit: scan_limit(limit),
                cursor: None,
            },
            filter: memory_filter(tags, text_contains),
            ..MemoryFindRequest::default()
        };
        let result = find.execute(&request).await?;
        Ok(take_visible(result.nodes, &visible, "find", limit, widen))
    }

    async fn ensure_branch(&self) -> Result<()> {
        let Some(row) = self.read_head().await? else {
            self.write_branch("main", None).await?;
            self.set_head_branch("main").await?;
            return Ok(());
        };

        if let Some(name) = row
            .get("branch")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            if self.read_branch(name).await?.is_none() {
                let tip = optional_string(row.get("commit_id"));
                self.write_branch(name, tip.as_deref()).await?;
            }
            return Ok(());
        }

        let tip = optional_string(row.get("commit_id"));
        self.write_branch("main", tip.as_deref()).await?;
        self.set_head_branch("main").await?;
        Ok(())
    }

    async fn read_head(&self) -> Result<Option<Value>> {
        let rows = self
            .client
            .raw_query("SELECT * FROM mori_ref:HEAD;", QueryParams::new())
            .await
            .context("reading HEAD")?;
        Ok(rows.into_iter().next())
    }

    async fn read_branch(&self, name: &str) -> Result<Option<BranchRow>> {
        let name = validate_branch_name(name)?;
        let rows = self
            .client
            .raw_query(
                &format!("SELECT * FROM mori_branch:`{name}`;"),
                QueryParams::new(),
            )
            .await
            .context("reading branch")?;
        Ok(rows.first().and_then(branch_from_row))
    }

    async fn write_branch(&self, name: &str, commit_id: Option<&str>) -> Result<()> {
        let name = validate_branch_name(name)?;
        let mut params = QueryParams::new();
        params.insert("mori_name".to_string(), json!(name));
        params.insert("mori_commit_id".to_string(), json!(commit_id));
        self.client
            .raw_query(
                &format!(
                    "UPSERT mori_branch:`{name}` SET name = $mori_name, commit_id = $mori_commit_id;"
                ),
                params,
            )
            .await
            .context("writing branch")?;
        Ok(())
    }

    async fn set_head_branch(&self, name: &str) -> Result<()> {
        let name = validate_branch_name(name)?;
        let mut params = QueryParams::new();
        params.insert("mori_branch".to_string(), json!(name));
        self.client
            .raw_query("UPSERT mori_ref:HEAD SET branch = $mori_branch;", params)
            .await
            .context("updating HEAD")?;
        Ok(())
    }

    async fn visible_nodes(&self) -> Result<VisibleNodes> {
        let mut visible = VisibleNodes::default();
        let head = self.branch_tip(&self.current_branch().await?).await?;
        for record in self.walk_history(head.as_deref(), None).await? {
            for entry in &record.entries {
                if let Some(identity) = self.node_identity(&entry.node_id).await? {
                    if !identity.sync_key.is_empty() {
                        visible.sync_keys.insert(identity.sync_key.clone());
                        visible
                            .source_by_sync
                            .entry(identity.sync_key)
                            .or_insert_with(|| entry.source.clone());
                    }
                    if !identity.raw.is_empty() {
                        visible.raws.insert(identity.raw.clone());
                        visible
                            .source_by_raw
                            .entry(identity.raw)
                            .or_insert_with(|| entry.source.clone());
                    }
                }
            }
        }
        Ok(visible)
    }

    async fn store_commit(&self, record: &CommitRecord) -> Result<()> {
        let mut params = QueryParams::new();
        params.insert("mori_commit_id".to_string(), json!(record.id));
        params.insert("mori_parent".to_string(), json!(record.parent));
        params.insert(
            "mori_merge_parents".to_string(),
            json!(record.merge_parents),
        );
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
                    "CREATE mori_commit:`{}` SET commit_id = $mori_commit_id, parent = $mori_parent, merge_parents = $mori_merge_parents, message = $mori_message, session = $mori_scope, created_at = $mori_created_at, entries = $mori_entries;",
                    record.id
                ),
                params,
            )
            .await
            .context("writing commit")?;
        Ok(())
    }

    /// `ancestor` is an ancestor of `descendant` when walking every parent of
    /// `descendant` reaches it. A missing commit is an ancestor of every history.
    async fn is_ancestor(&self, ancestor: Option<&str>, descendant: Option<&str>) -> Result<bool> {
        let Some(ancestor) = ancestor else {
            return Ok(true);
        };
        Ok(self.reachable(descendant).await?.contains(ancestor))
    }

    async fn reachable(&self, start: Option<&str>) -> Result<HashSet<String>> {
        Ok(self
            .walk_history(start, None)
            .await?
            .into_iter()
            .map(|record| record.id)
            .collect())
    }

    /// Newest first. Stops when a commit is already in `stop`, which is the
    /// history being replayed onto.
    async fn first_parent_commits(
        &self,
        start: Option<&str>,
        stop: &HashSet<String>,
    ) -> Result<Vec<CommitRecord>> {
        let mut records = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = start.map(str::to_string);
        while let Some(id) = cursor {
            if stop.contains(&id) || !seen.insert(id.clone()) {
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

    /// Newest first. Merge parents are pushed before the first parent so the
    /// first parent is visited next, then the merged-in history.
    async fn walk_history(
        &self,
        start: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<CommitRecord>> {
        let Some(start) = start else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![start.to_string()];
        while let Some(id) = stack.pop() {
            if limit.is_some_and(|limit| records.len() >= limit) {
                break;
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(record) = self.commit_by_id(&id).await? else {
                continue;
            };
            for parent in record.merge_parents.iter().rev() {
                stack.push(parent.clone());
            }
            if let Some(parent) = &record.parent {
                stack.push(parent.clone());
            }
            records.push(record);
        }
        Ok(records)
    }

    async fn node_identity(&self, node_id: &str) -> Result<Option<NodeIdentity>> {
        let id = sanitize_node_id(node_id)?;
        let rows = self
            .client
            .raw_query(
                &format!("SELECT raw, sync_key FROM temporal_node:`{id}`;"),
                QueryParams::new(),
            )
            .await
            .context("reading stored node identity")?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        Ok(Some(NodeIdentity {
            sync_key: optional_string(row.get("sync_key")).unwrap_or_default(),
            raw: optional_string(row.get("raw")).unwrap_or_default(),
        }))
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

#[derive(Debug, Clone)]
pub struct BranchInfo {
    pub name: String,
    pub commit_id: Option<String>,
    pub current: bool,
}

struct BranchRow {
    name: String,
    commit_id: Option<String>,
}

#[derive(Default)]
struct VisibleNodes {
    sync_keys: HashSet<String>,
    raws: HashSet<String>,
    source_by_sync: HashMap<String, String>,
    source_by_raw: HashMap<String, String>,
}

impl VisibleNodes {
    fn is_empty(&self) -> bool {
        self.sync_keys.is_empty() && self.raws.is_empty()
    }

    fn matches(&self, sync_key: &str, raw: &str) -> bool {
        (!sync_key.is_empty() && self.sync_keys.contains(sync_key))
            || (!raw.is_empty() && self.raws.contains(raw))
    }

    fn source_for(&self, sync_key: &str, raw: &str) -> String {
        if !sync_key.is_empty() {
            if let Some(source) = self.source_by_sync.get(sync_key) {
                return source.clone();
            }
        }
        if !raw.is_empty() {
            if let Some(source) = self.source_by_raw.get(raw) {
                return source.clone();
            }
        }
        String::new()
    }
}

struct NodeIdentity {
    sync_key: String,
    raw: String,
}

fn scan_limit(limit: usize) -> usize {
    limit.saturating_mul(25).clamp(64, 2000)
}

fn validate_branch_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.eq_ignore_ascii_case("head") {
        bail!("HEAD is reserved");
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("branch name cannot be empty");
    };
    let rest_ok = chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-');
    if name.len() > 64 || !first.is_ascii_alphanumeric() || !rest_ok {
        bail!("branch names use letters, digits, '.', '_' and '-'");
    }
    Ok(name)
}

fn sanitize_node_id(node_id: &str) -> Result<String> {
    let id = node_id
        .trim()
        .trim_start_matches("temporal_node:")
        .replace('`', "");
    if id.is_empty() || id.contains(';') || id.contains(' ') {
        bail!("invalid node id");
    }
    Ok(id)
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "null" {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn branch_from_row(row: &Value) -> Option<BranchRow> {
    let name = optional_string(row.get("name")).or_else(|| {
        optional_string(row.get("id")).and_then(|id| {
            id.rsplit([':', '`'])
                .find(|part| !part.is_empty() && *part != "mori_branch")
                .map(str::to_string)
        })
    })?;
    Some(BranchRow {
        name,
        commit_id: optional_string(row.get("commit_id")),
    })
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
    /// Facets from `mori add --tag`. Empty on commits written before tags existed.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub id: String,
    /// First parent. Ordinary commits have only this one.
    pub parent: Option<String>,
    /// Extra parents written by merge. Empty on commits that are not merges.
    #[serde(default)]
    pub merge_parents: Vec<String>,
    pub message: String,
    pub session: String,
    pub created_at: DateTime<Utc>,
    pub entries: Vec<CommitEntry>,
}

#[derive(Debug, Clone)]
pub enum MergeOutcome {
    UpToDate,
    FastForward { commit_id: String },
    Merged { commit: CommitRecord },
}

#[derive(Debug, Clone)]
pub enum RebaseOutcome {
    UpToDate,
    FastForward {
        commit_id: String,
    },
    Rebased {
        commits: Vec<CommitRecord>,
        tip: String,
    },
}

#[derive(Debug, Clone)]
pub struct Recalled {
    pub session: String,
    pub summary: String,
    pub raw_sttp: String,
    pub path: String,
    /// Path passed to `mori add`, when this node is on the current branch.
    pub source: String,
}

fn take_visible(
    nodes: Vec<locus_core_rs::domain::models::SttpNode>,
    visible: &VisibleNodes,
    path: &str,
    limit: usize,
    widen: bool,
) -> Vec<Recalled> {
    let cap = if widen { scan_limit(limit) } else { limit };
    nodes
        .into_iter()
        .filter(|node| visible.matches(node.sync_key.as_str(), node.raw.as_str()))
        .take(cap)
        .map(|node| Recalled {
            source: visible.source_for(node.sync_key.as_str(), node.raw.as_str()),
            session: node.session_id,
            summary: node
                .context_summary
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "(stored context)".to_string()),
            raw_sttp: node.raw,
            path: path.to_string(),
        })
        .collect()
}

/// A one-content-term query never reaches Locus lexical fallback, because the
/// resonance set is not empty. Constrain that query with `text_contains` so
/// `mori recall zebra` does not return every note on the branch.
fn single_content_term(query: &str) -> Option<String> {
    let parsed = parse_lexical_query(query);
    if parsed.term_count() != 1 {
        return None;
    }
    parsed
        .coverage
        .first()
        .and_then(|variants| variants.first().cloned())
}

fn memory_filter(tags: &[String], text_contains: Option<String>) -> MemoryFilter {
    // `indexed_tags` is the index-backed AND, but the pinned SurrealKV build
    // rejects that query (`HAVING` is a parse error). `tags_contains` ANDs the
    // same semantic tags on the nodes recall and find already load.
    let tags_contains = if tags.is_empty() {
        None
    } else {
        Some(tags.to_vec())
    };
    MemoryFilter {
        text_contains,
        tags_contains,
        ..MemoryFilter::default()
    }
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
    let merge_parents = row
        .get("merge_parents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|value| match value {
                    Value::String(text) if !text.is_empty() && text != "null" => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(CommitRecord {
        id,
        parent,
        merge_parents,
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

#[cfg(test)]
mod tests {
    use super::single_content_term;

    #[test]
    fn one_content_term_queries_are_marked_for_filtering() {
        assert_eq!(single_content_term("zebra").as_deref(), Some("zebra"));
        assert_eq!(single_content_term("  Zebra ").as_deref(), Some("zebra"));
        assert_eq!(
            single_content_term("the orchard").as_deref(),
            Some("orchard")
        );
        assert_eq!(single_content_term("orchard plan"), None);
        assert_eq!(single_content_term("the"), None);
    }
}
