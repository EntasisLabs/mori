use std::sync::Arc;

use anyhow::{bail, Result};
use async_trait::async_trait;
use locus_core_rs::domain::contracts::{NodeStore, SyncChangeSource};
use locus_core_rs::domain::models::{
    ChangeQueryResult, NodeQuery, SttpNode, SyncCursor, SyncPullRequest,
};
use locus_core_rs::SyncCoordinatorService;

use crate::memory::{CommitRecord, Memory};

/// Nodes travel through Locus `SyncCoordinatorService`. Commits travel with
/// them, and each entry's node id is rewritten to the destination record
/// because upsert identifies a node by sync key, not by its storage id.
pub async fn fetch_remote(
    local: &Memory,
    remote: &Memory,
    remote_name: &str,
) -> Result<FetchReport> {
    let nodes = pull_nodes(local, remote, &format!("fetch:{remote_name}")).await?;
    let commits = copy_commits(remote, local, &remote.list_commits().await?).await?;
    let mut branches = Vec::new();
    for branch in remote.branches().await? {
        if branch.name.contains('/') {
            continue;
        }
        let tracking = format!("{remote_name}/{}", branch.name);
        local
            .write_branch(&tracking, branch.commit_id.as_deref())
            .await?;
        branches.push((tracking, branch.commit_id));
    }
    Ok(FetchReport {
        remote: remote_name.to_string(),
        fetched: nodes.fetched,
        created: nodes.created,
        updated: nodes.updated,
        commits,
        branches,
    })
}

pub async fn push_remote(
    local: &Memory,
    remote: &Memory,
    remote_name: &str,
    repo_id: &str,
) -> Result<PushReport> {
    let branch = local.current_branch().await?;
    if branch.contains('/') {
        bail!("check out a local branch before pushing");
    }
    let nodes = pull_nodes(remote, local, &format!("push:{repo_id}")).await?;
    let local_tip = local.branch_tip(&branch).await?;
    let history = local.walk_history(local_tip.as_deref(), None).await?;
    let commits = copy_commits(local, remote, &history).await?;

    let outcome = match local_tip {
        None => PushOutcome::UpToDate,
        Some(local_tip) => {
            let existing = remote
                .branches()
                .await?
                .into_iter()
                .find(|candidate| candidate.name == branch);
            let remote_tip = existing.as_ref().and_then(|row| row.commit_id.clone());
            if remote
                .is_ancestor(Some(&local_tip), remote_tip.as_deref())
                .await?
            {
                PushOutcome::UpToDate
            } else if remote
                .is_ancestor(remote_tip.as_deref(), Some(&local_tip))
                .await?
            {
                remote.write_branch(&branch, Some(&local_tip)).await?;
                if existing.is_none() {
                    PushOutcome::Created {
                        commit_id: local_tip,
                    }
                } else {
                    PushOutcome::FastForward {
                        commit_id: local_tip,
                    }
                }
            } else {
                bail!("push rejected: {branch} on {remote_name} has diverged");
            }
        }
    };

    Ok(PushReport {
        remote: remote_name.to_string(),
        branch,
        fetched: nodes.fetched,
        created: nodes.created,
        updated: nodes.updated,
        commits,
        outcome,
    })
}

/// Fetch, fast-forward the current branch when the remote is strictly ahead,
/// then push. A diverged history is reported and left for `merge`.
pub async fn sync_remote(
    local: &Memory,
    remote: &Memory,
    remote_name: &str,
    repo_id: &str,
) -> Result<SyncReport> {
    let fetch = fetch_remote(local, remote, remote_name).await?;
    let branch = local.current_branch().await?;
    let tracking = format!("{remote_name}/{branch}");
    let local_tip = local.branch_tip(&branch).await?;
    let tracking_tip = local
        .branches()
        .await?
        .into_iter()
        .find(|candidate| candidate.name == tracking)
        .and_then(|candidate| candidate.commit_id);

    let remote_is_ahead = local
        .is_ancestor(local_tip.as_deref(), tracking_tip.as_deref())
        .await?
        && local_tip != tracking_tip;
    let local_is_ahead = local
        .is_ancestor(tracking_tip.as_deref(), local_tip.as_deref())
        .await?;
    if remote_is_ahead {
        if let Some(tip) = tracking_tip.clone() {
            local.write_branch(&branch, Some(&tip)).await?;
        }
    }
    let diverged = tracking_tip.is_some() && !remote_is_ahead && !local_is_ahead;
    let push = if diverged {
        None
    } else {
        Some(push_remote(local, remote, remote_name, repo_id).await?)
    };

    Ok(SyncReport {
        fetch,
        branch,
        fast_forwarded_to: remote_is_ahead.then_some(tracking_tip).flatten(),
        push,
        diverged,
    })
}

#[derive(Debug, Clone)]
pub struct FetchReport {
    pub remote: String,
    pub fetched: usize,
    pub created: usize,
    pub updated: usize,
    pub commits: usize,
    pub branches: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct PushReport {
    pub remote: String,
    pub branch: String,
    pub fetched: usize,
    pub created: usize,
    pub updated: usize,
    pub commits: usize,
    pub outcome: PushOutcome,
}

#[derive(Debug, Clone)]
pub enum PushOutcome {
    UpToDate,
    FastForward { commit_id: String },
    Created { commit_id: String },
}

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub fetch: FetchReport,
    pub branch: String,
    pub fast_forwarded_to: Option<String>,
    pub push: Option<PushReport>,
    pub diverged: bool,
}

#[derive(Default)]
struct NodePullStats {
    fetched: usize,
    created: usize,
    updated: usize,
}

struct StoreChangeSource {
    store: Arc<dyn NodeStore>,
}

#[async_trait]
impl SyncChangeSource for StoreChangeSource {
    async fn read_changes_async(
        &self,
        session_id: &str,
        _connector_id: &str,
        cursor: Option<SyncCursor>,
        limit: usize,
    ) -> Result<ChangeQueryResult> {
        // `NodeStore::query_changes_since_async` emits `NOT $include_cursor`,
        // which SurrealDB 3.3 rejects. Page the public node query and apply
        // the same updated_at/sync_key cursor here instead.
        let scanned = self
            .store
            .query_nodes_async(NodeQuery {
                limit: 5000,
                session_id: Some(session_id.to_string()),
                from_utc: None,
                to_utc: None,
                tiers: None,
            })
            .await?;
        let mut nodes = scanned
            .into_iter()
            .filter(|node| after_cursor(node, cursor.as_ref()))
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            left.updated_at
                .cmp(&right.updated_at)
                .then_with(|| left.sync_key.cmp(&right.sync_key))
        });
        let limit = limit.max(1);
        let has_more = nodes.len() > limit;
        if has_more {
            nodes.truncate(limit);
        }
        let next_cursor = nodes.last().map(|node| SyncCursor {
            updated_at: node.updated_at,
            sync_key: node.sync_key.clone(),
        });
        Ok(ChangeQueryResult {
            nodes,
            next_cursor,
            has_more,
        })
    }
}

fn after_cursor(node: &SttpNode, cursor: Option<&SyncCursor>) -> bool {
    let Some(cursor) = cursor else {
        return true;
    };
    node.updated_at > cursor.updated_at
        || (node.updated_at == cursor.updated_at && node.sync_key > cursor.sync_key)
}

async fn pull_nodes(dest: &Memory, source: &Memory, connector_id: &str) -> Result<NodePullStats> {
    let coordinator = SyncCoordinatorService::new(
        dest.node_store(),
        Arc::new(StoreChangeSource {
            store: source.node_store(),
        }),
    );
    let mut stats = NodePullStats::default();
    for session in source.list_sessions().await? {
        let result = coordinator
            .pull_async(SyncPullRequest {
                session_id: session,
                connector_id: connector_id.to_string(),
                page_size: 200,
                max_batches: None,
            })
            .await?;
        stats.fetched += result.fetched;
        stats.created += result.created;
        stats.updated += result.updated;
    }
    Ok(stats)
}

async fn copy_commits(source: &Memory, dest: &Memory, commits: &[CommitRecord]) -> Result<usize> {
    let mut copied = 0;
    for record in commits.iter().rev() {
        if dest.commit_by_id(&record.id).await?.is_some() {
            continue;
        }
        let mut stored = record.clone();
        for entry in &mut stored.entries {
            let Some(sync_key) = source.lookup_sync_key(&entry.node_id).await? else {
                bail!(
                    "commit {} is missing the stored node for {}",
                    &record.id[..record.id.len().min(12)],
                    entry.source
                );
            };
            let Some(node_id) = dest.lookup_node_id(&entry.session, &sync_key).await? else {
                bail!("node {} did not arrive on the destination", entry.source);
            };
            entry.node_id = node_id;
        }
        dest.store_commit(&stored).await?;
        copied += 1;
    }
    Ok(copied)
}
