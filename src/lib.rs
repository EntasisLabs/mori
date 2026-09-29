//! Mori is a git-shaped interface over Locus memory.
//!
//! Context goes in as documents, chats, notes, or raw STTP. The Locus SDK
//! compiles the non-STTP forms. Persistence is an embedded SurrealKV file
//! opened for one command and dropped when that command finishes.

mod compile;
mod memory;
mod repo;
mod stage;
mod stash;
mod sync;

pub use compile::{compile_context, detect_kind, CompiledContext};
pub use memory::{BranchInfo, Memory, MergeOutcome, RebaseOutcome, Recalled};
pub use repo::{
    find_repo, init_repo, validate_remote_endpoint, validate_remote_name, RemoteConfig, Repo,
    RepoConfig,
};
pub use stage::{read_source, ContextKind, StageIndex, StagedContext};
pub use stash::{Stash, StashEntry};
pub use sync::{
    fetch_remote, push_remote, sync_remote, FetchReport, PushOutcome, PushReport, SyncReport,
};
