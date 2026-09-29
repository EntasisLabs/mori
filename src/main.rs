use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use mori::{
    compile_context, detect_kind, find_repo, init_repo, read_source, ContextKind, Memory, Repo,
    StageIndex, StagedContext, Stash,
};
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "mori",
    version,
    about = "the git for cognition",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Auto,
    Sttp,
    Document,
    Chat,
    Note,
    Context,
}

impl From<KindArg> for ContextKind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::Auto => ContextKind::Auto,
            KindArg::Sttp => ContextKind::Sttp,
            KindArg::Document => ContextKind::Document,
            KindArg::Chat => ContextKind::Chat,
            KindArg::Note => ContextKind::Note,
            KindArg::Context => ContextKind::Context,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Create a cognition repo in this directory.
    Init {
        /// Directory to initialize. Defaults to the current directory.
        path: Option<PathBuf>,
        /// Default Locus session label stored on new context.
        #[arg(long, default_value = "main")]
        session: String,
    },
    /// Stage a document, chat, note, or raw STTP file.
    Add {
        /// Files to stage. Use `-` to read stdin.
        sources: Vec<String>,
        #[arg(long, value_enum, default_value = "auto")]
        kind: KindArg,
        #[arg(long)]
        session: Option<String>,
    },
    /// Show the branch, HEAD, staged context, and stash.
    Status,
    /// List branches, or create one at the current tip.
    Branch {
        /// New branch name. Omit it to list branches.
        name: Option<String>,
    },
    /// Check out a branch. Refuses when context is staged.
    Checkout {
        name: String,
        /// Create the branch at the current tip, then check it out.
        #[arg(short = 'b')]
        branch: bool,
    },
    /// Park staged context so you can check out another branch.
    Stash {
        /// Message recorded on the stash. Used when parking context.
        #[arg(short, long)]
        message: Option<String>,
        #[command(subcommand)]
        action: Option<StashCommand>,
    },
    /// Compile context into STTP and print it. Does not open the database.
    Compile {
        /// Files to compile. With no paths, compile whatever is staged.
        sources: Vec<String>,
        #[arg(long, value_enum, default_value = "auto")]
        kind: KindArg,
        #[arg(long)]
        session: Option<String>,
    },
    /// Compile staged context and store it in this repo's SurrealKV file.
    Commit {
        #[arg(short, long)]
        message: String,
    },
    /// List stored commits, newest first.
    Log {
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show one commit. Pass `--raw` to print the stored STTP.
    Show {
        rev: String,
        #[arg(long)]
        raw: bool,
    },
    /// Rank stored context against a question.
    Recall {
        query: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long, default_value_t = 8)]
        limit: usize,
        #[arg(long)]
        raw: bool,
    },
    /// Filter stored context without ranking.
    Find {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        contains: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Unstage context. With no paths, unstage everything.
    Reset { sources: Vec<String> },
}

#[derive(Subcommand)]
enum StashCommand {
    /// Show parked context, newest first.
    List,
    /// Restore the newest stash onto an empty index.
    Pop,
    /// Drop the newest stash.
    Drop,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mori: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { path, session } => cmd_init(path, &session).await,
        Command::Add {
            sources,
            kind,
            session,
        } => cmd_add(sources, kind.into(), session),
        Command::Status => cmd_status().await,
        Command::Branch { name } => cmd_branch(name).await,
        Command::Checkout { name, branch } => cmd_checkout(&name, branch).await,
        Command::Stash { message, action } => cmd_stash(message, action).await,
        Command::Compile {
            sources,
            kind,
            session,
        } => cmd_compile(sources, kind.into(), session),
        Command::Commit { message } => cmd_commit(&message).await,
        Command::Log { limit } => cmd_log(limit).await,
        Command::Show { rev, raw } => cmd_show(&rev, raw).await,
        Command::Recall {
            query,
            session,
            limit,
            raw,
        } => cmd_recall(&query, session.as_deref(), limit, raw).await,
        Command::Find {
            session,
            contains,
            limit,
        } => cmd_find(session.as_deref(), contains.as_deref(), limit).await,
        Command::Reset { sources } => cmd_reset(sources),
    }
}

async fn cmd_init(path: Option<PathBuf>, session: &str) -> Result<()> {
    let path = path.unwrap_or(std::env::current_dir()?);
    let repo = init_repo(&path, session)?;
    let memory = Memory::connect(&repo).await?;
    let endpoint = memory.endpoint().to_string();
    memory.disconnect();
    println!(
        "initialized empty mori repository in {}",
        repo.mori_dir().display()
    );
    println!("branch main");
    println!("session {session}");
    println!("store {endpoint}");
    Ok(())
}

fn cmd_add(sources: Vec<String>, kind: ContextKind, session: Option<String>) -> Result<()> {
    if sources.is_empty() {
        bail!("nothing specified, nothing added");
    }
    let repo = repo_from_cwd()?;
    let config = repo.load_config()?;
    let session = session.unwrap_or(config.default_session);
    let mut index = StageIndex::load(&repo)?;
    for source in sources {
        let text = read_source(&source)?;
        let resolved = match kind {
            ContextKind::Auto => detect_kind(&source, &text),
            other => other,
        };
        index.upsert(StagedContext {
            id: Uuid::new_v4().to_string(),
            kind: resolved,
            session: session.clone(),
            source,
            text,
            added_at: Utc::now(),
        });
    }
    let staged = index.entries().to_vec();
    index.save(&repo)?;
    for entry in staged {
        println!("staged {} {}", entry.kind.as_str(), entry.source);
    }
    Ok(())
}

async fn cmd_status() -> Result<()> {
    let repo = repo_from_cwd()?;
    let config = repo.load_config()?;
    let index = StageIndex::load(&repo)?;
    println!("session {}", config.default_session);

    if repo.kv_dir().exists() {
        let memory = Memory::connect(&repo).await?;
        let branch = memory.current_branch().await?;
        println!("On branch {branch}");
        match memory.head().await? {
            Some(head) => println!("HEAD {} {}", short(&head.id), head.message),
            None => println!("HEAD (no commits yet)"),
        }
        memory.disconnect();
    } else {
        println!("On branch main");
        println!("HEAD (no commits yet)");
    }

    let stash = Stash::load(&repo)?;
    if !stash.is_empty() {
        println!(
            "stash: {} {}",
            stash.entries().len(),
            if stash.entries().len() == 1 {
                "entry"
            } else {
                "entries"
            }
        );
    }

    if index.is_empty() {
        println!("nothing staged");
    } else {
        println!("staged context:");
        for entry in index.entries() {
            println!(
                "  {:<10} {}  ({})",
                entry.kind.as_str(),
                entry.source,
                entry.session
            );
        }
    }
    Ok(())
}

fn cmd_compile(sources: Vec<String>, kind: ContextKind, session: Option<String>) -> Result<()> {
    let staged = if sources.is_empty() {
        let repo = repo_from_cwd()?;
        StageIndex::load(&repo)?.entries().to_vec()
    } else {
        let session = match session {
            Some(session) => session,
            None => match find_repo(&std::env::current_dir()?) {
                Ok(repo) => repo.load_config()?.default_session,
                Err(_) => "main".to_string(),
            },
        };
        sources
            .into_iter()
            .map(|source| {
                let text = read_source(&source)?;
                Ok(StagedContext {
                    id: Uuid::new_v4().to_string(),
                    kind,
                    session: session.clone(),
                    source,
                    text,
                    added_at: Utc::now(),
                })
            })
            .collect::<Result<Vec<_>>>()?
    };

    if staged.is_empty() {
        bail!("nothing to compile");
    }

    for entry in &staged {
        let compiled = compile_context(entry)?;
        eprintln!(
            "compiled {} {} (session {})",
            compiled.kind.as_str(),
            compiled.source,
            compiled.session
        );
        println!("{}", compiled.raw_sttp);
    }
    Ok(())
}

async fn cmd_commit(message: &str) -> Result<()> {
    let repo = repo_from_cwd()?;
    let config = repo.load_config()?;
    let index = StageIndex::load(&repo)?;
    if index.is_empty() {
        bail!("nothing staged");
    }
    let memory = Memory::connect(&repo).await?;
    let branch = memory.current_branch().await?;
    let record = memory
        .commit(message, &config.default_session, index.entries())
        .await?;
    memory.disconnect();

    let mut cleared = index;
    cleared.clear();
    cleared.save(&repo)?;

    println!("[{} {}] {}", branch, short(&record.id), record.message);
    println!(" {} context stored", record.entries.len());
    for entry in &record.entries {
        println!("  {} {}", entry.kind.as_str(), entry.source);
    }
    Ok(())
}

async fn cmd_log(limit: usize) -> Result<()> {
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let branch = memory.current_branch().await?;
    let records = memory.log(limit).await?;
    memory.disconnect();
    println!("branch {branch}");
    if records.is_empty() {
        println!("no commits yet");
        return Ok(());
    }
    for record in records {
        println!("commit {}", record.id);
        println!("session {}", record.session);
        println!("Date: {}", record.created_at.to_rfc3339());
        println!();
        println!("    {}", record.message);
        println!();
        for entry in &record.entries {
            println!("    {} {}", entry.kind.as_str(), entry.source);
            println!("    {}", entry.summary);
        }
        println!();
    }
    Ok(())
}

async fn cmd_show(rev: &str, raw: bool) -> Result<()> {
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let record = memory.find_commit(rev).await?;
    println!("commit {}", record.id);
    println!("session {}", record.session);
    println!("Date: {}", record.created_at.to_rfc3339());
    println!();
    println!("    {}", record.message);
    println!();
    for entry in &record.entries {
        println!("    {} {}", entry.kind.as_str(), entry.source);
        println!("    {}", entry.summary);
        if raw {
            let text = memory.node_raw(&entry.node_id).await?;
            println!();
            println!("{text}");
            println!();
        }
    }
    memory.disconnect();
    Ok(())
}

async fn cmd_recall(query: &str, session: Option<&str>, limit: usize, raw: bool) -> Result<()> {
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let hits = memory.recall(query, session, limit).await?;
    memory.disconnect();
    if hits.is_empty() {
        println!("nothing recalled");
        return Ok(());
    }
    let path = hits
        .first()
        .map(|hit| hit.path.as_str())
        .unwrap_or("recall");
    println!("retrieved {} via {path}", hits.len());
    for hit in hits {
        println!("{}", hit.summary);
        if raw {
            println!("{}", hit.raw_sttp);
        }
    }
    Ok(())
}

async fn cmd_find(session: Option<&str>, contains: Option<&str>, limit: usize) -> Result<()> {
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let hits = memory.find(session, contains, limit).await?;
    memory.disconnect();
    if hits.is_empty() {
        println!("nothing found");
        return Ok(());
    }
    println!("found {}", hits.len());
    for hit in hits {
        println!("{}", hit.summary);
    }
    Ok(())
}

async fn cmd_branch(name: Option<String>) -> Result<()> {
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    if let Some(name) = name {
        let created = memory.create_branch(&name).await?;
        memory.disconnect();
        println!("created branch {created}");
        return Ok(());
    }

    let branches = memory.branches().await?;
    memory.disconnect();
    if branches.is_empty() {
        println!("no branches yet");
        return Ok(());
    }
    for branch in branches {
        let marker = if branch.current { "*" } else { " " };
        match branch.commit_id.as_deref() {
            Some(id) => println!("{marker} {} {}", branch.name, short(id)),
            None => println!("{marker} {}", branch.name),
        }
    }
    Ok(())
}

async fn cmd_checkout(name: &str, create: bool) -> Result<()> {
    let repo = repo_from_cwd()?;
    let index = StageIndex::load(&repo)?;
    let memory = Memory::connect(&repo).await?;
    let current = memory.current_branch().await?;
    if current != name && !index.is_empty() {
        bail!(
            "staged context is in the way; stash or commit it before checking out another branch"
        );
    }
    if create {
        memory.create_branch(name).await?;
    }
    memory.checkout_branch(name).await?;
    memory.disconnect();
    if create {
        println!("checked out new branch {name}");
    } else {
        println!("checked out branch {name}");
    }
    Ok(())
}

async fn cmd_stash(message: Option<String>, action: Option<StashCommand>) -> Result<()> {
    match action {
        Some(StashCommand::List) => cmd_stash_list(),
        Some(StashCommand::Pop) => cmd_stash_pop(),
        Some(StashCommand::Drop) => cmd_stash_drop(),
        None => cmd_stash_push(message.as_deref()).await,
    }
}

async fn cmd_stash_push(message: Option<&str>) -> Result<()> {
    let repo = repo_from_cwd()?;
    let mut index = StageIndex::load(&repo)?;
    if index.is_empty() {
        bail!("nothing staged");
    }
    let memory = Memory::connect(&repo).await?;
    let branch = memory.current_branch().await?;
    memory.disconnect();

    let mut stash = Stash::load(&repo)?;
    let entry = stash
        .push(&branch, message.unwrap_or(""), index.entries().to_vec())
        .clone();
    stash.save(&repo)?;
    index.clear();
    index.save(&repo)?;
    println!("stashed {}: {}", short(&entry.id), entry.message);
    Ok(())
}

fn cmd_stash_list() -> Result<()> {
    let repo = repo_from_cwd()?;
    let stash = Stash::load(&repo)?;
    if stash.is_empty() {
        println!("nothing stashed");
        return Ok(());
    }
    for entry in stash.entries() {
        println!(
            "stash {}: {} (branch {})",
            short(&entry.id),
            entry.message,
            entry.branch
        );
    }
    Ok(())
}

fn cmd_stash_pop() -> Result<()> {
    let repo = repo_from_cwd()?;
    let mut index = StageIndex::load(&repo)?;
    if !index.is_empty() {
        bail!("staged context is in the way; commit, reset, or stash it before popping");
    }
    let mut stash = Stash::load(&repo)?;
    let entry = stash.pop()?;
    index.replace(entry.entries);
    index.save(&repo)?;
    stash.save(&repo)?;
    println!(
        "restored stash {}: {} (branch {})",
        short(&entry.id),
        entry.message,
        entry.branch
    );
    Ok(())
}

fn cmd_stash_drop() -> Result<()> {
    let repo = repo_from_cwd()?;
    let mut stash = Stash::load(&repo)?;
    let entry = stash.pop()?;
    stash.save(&repo)?;
    println!("dropped stash {}: {}", short(&entry.id), entry.message);
    Ok(())
}

fn cmd_reset(sources: Vec<String>) -> Result<()> {
    let repo = repo_from_cwd()?;
    let mut index = StageIndex::load(&repo)?;
    if sources.is_empty() {
        index.clear();
        index.save(&repo)?;
        println!("unstaged all context");
        return Ok(());
    }
    for source in &sources {
        if !index.unstage(source) {
            bail!("{source} is not staged");
        }
        println!("unstaged {source}");
    }
    index.save(&repo)?;
    Ok(())
}

fn repo_from_cwd() -> Result<Repo> {
    find_repo(&std::env::current_dir().context("current directory")?)
}

fn short(id: &str) -> &str {
    let end = id.chars().take(12).map(|ch| ch.len_utf8()).sum();
    &id[..end]
}
