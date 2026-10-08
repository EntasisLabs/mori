use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use mori::{
    compile_context, detect_kind, find_repo, init_repo, normalize_tags, read_source, view_hits,
    ContextKind, MatchPattern, Memory, MergeOutcome, RebaseOutcome, Recalled, Repo, StageIndex,
    StagedContext, Stash, ViewOptions, ViewedHit,
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

#[derive(Args, Clone)]
struct TagArgs {
    /// Facet. Repeat for several, or pass a comma-separated list.
    #[arg(long = "tag", value_name = "TAG")]
    tag: Vec<String>,
}

#[derive(Args, Clone)]
struct CutArgs {
    /// Print matching sections (path and heading) instead of the summary line.
    #[arg(long)]
    excerpt: bool,
    /// Print the stored text of each hit instead of the summary line.
    #[arg(long)]
    full: bool,
    /// Keep hits whose text matches this regular expression, and use it to choose sections.
    #[arg(long = "match", value_name = "PATTERN")]
    pattern: Option<String>,
    /// With --excerpt, lines of context around each match instead of the whole section.
    #[arg(short = 'C', long = "context", value_name = "N")]
    context_lines: Option<usize>,
}

#[derive(Clone, Copy)]
enum DisplayMode {
    Summary,
    Excerpt,
    Full,
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
    /// Stage a freeform note. It stays in the index until commit.
    ///
    /// `-m` sets the text. A pipe, or `-`, reads stdin.
    Note {
        /// Pass `-` to read stdin.
        #[arg(value_name = "-")]
        stdin: Option<String>,
        /// Note text.
        #[arg(short, long, value_name = "TEXT")]
        message: Option<String>,
        /// Session label. Defaults to the repo session.
        #[arg(long)]
        session: Option<String>,
        #[command(flatten)]
        tags: TagArgs,
    },
    /// Stage a document, chat, note, or raw STTP file.
    Add {
        /// Files to stage. Use `-` to read stdin.
        sources: Vec<String>,
        #[arg(long, value_enum, default_value = "auto")]
        kind: KindArg,
        #[arg(long)]
        session: Option<String>,
        #[command(flatten)]
        tags: TagArgs,
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
    /// Bring another branch into the current one.
    Merge {
        branch: String,
        /// Message for a merge commit. Fast-forwards do not use it.
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Replay the current branch's commits onto another branch.
    Rebase {
        /// Branch to replay onto. That branch is left where it is.
        onto: String,
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
        #[command(flatten)]
        tags: TagArgs,
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
    ///
    /// Prints one summary line per hit. `--excerpt` prints matching sections,
    /// `--full` prints stored text, and `--raw` prints STTP.
    Recall {
        query: String,
        #[arg(long)]
        session: Option<String>,
        #[command(flatten)]
        tags: TagArgs,
        #[arg(long, default_value_t = 8)]
        limit: usize,
        #[command(flatten)]
        cut: CutArgs,
        #[arg(long)]
        raw: bool,
    },
    /// Filter stored context without ranking.
    Find {
        #[arg(long)]
        session: Option<String>,
        #[command(flatten)]
        tags: TagArgs,
        #[arg(long)]
        contains: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[command(flatten)]
        cut: CutArgs,
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
        Command::Note {
            stdin,
            message,
            session,
            tags,
        } => cmd_note(message, stdin.as_deref(), session, &tags.tag),
        Command::Add {
            sources,
            kind,
            session,
            tags,
        } => cmd_add(sources, kind.into(), session, &tags.tag),
        Command::Status => cmd_status().await,
        Command::Branch { name } => cmd_branch(name).await,
        Command::Checkout { name, branch } => cmd_checkout(&name, branch).await,
        Command::Merge { branch, message } => cmd_merge(&branch, message.as_deref()).await,
        Command::Rebase { onto } => cmd_rebase(&onto).await,
        Command::Stash { message, action } => cmd_stash(message, action).await,
        Command::Compile {
            sources,
            kind,
            session,
            tags,
        } => cmd_compile(sources, kind.into(), session, &tags.tag),
        Command::Commit { message } => cmd_commit(&message).await,
        Command::Log { limit } => cmd_log(limit).await,
        Command::Show { rev, raw } => cmd_show(&rev, raw).await,
        Command::Recall {
            query,
            session,
            tags,
            limit,
            cut,
            raw,
        } => cmd_recall(&query, session.as_deref(), &tags.tag, limit, &cut, raw).await,
        Command::Find {
            session,
            tags,
            contains,
            limit,
            cut,
        } => {
            cmd_find(
                session.as_deref(),
                &tags.tag,
                contains.as_deref(),
                limit,
                &cut,
            )
            .await
        }
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

fn cmd_add(
    sources: Vec<String>,
    kind: ContextKind,
    session: Option<String>,
    tags: &[String],
) -> Result<()> {
    if sources.is_empty() {
        bail!("nothing specified, nothing added");
    }
    let tags = normalize_tags(tags)?;
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
            tags: tags.clone(),
        });
    }
    let staged = index.entries().to_vec();
    index.save(&repo)?;
    for entry in staged {
        println!(
            "staged {} {}{}",
            entry.kind.as_str(),
            entry.source,
            tags_suffix(&entry.tags)
        );
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
                "  {:<10} {}  ({}){}",
                entry.kind.as_str(),
                entry.source,
                entry.session,
                tags_suffix(&entry.tags)
            );
            if entry.kind == ContextKind::Note {
                let preview = note_preview(&entry.text);
                if !preview.is_empty() {
                    println!("    {preview}");
                }
            }
        }
    }
    Ok(())
}

fn cmd_note(
    message: Option<String>,
    positional: Option<&str>,
    session: Option<String>,
    tags: &[String],
) -> Result<()> {
    let read_stdin = resolve_note_input(message.as_deref(), positional, io::stdin().is_terminal())?;
    let tags = normalize_tags(tags)?;
    let text = if read_stdin {
        read_source("-")?
    } else {
        message.unwrap_or_default()
    };
    let text = text.trim().to_string();
    if text.is_empty() {
        bail!("the note is empty");
    }

    let repo = repo_from_cwd()?;
    let config = repo.load_config()?;
    let session = session.unwrap_or(config.default_session);
    let mut index = StageIndex::load(&repo)?;
    index.upsert(StagedContext {
        id: Uuid::new_v4().to_string(),
        kind: ContextKind::Note,
        session,
        source: "-".to_string(),
        text,
        added_at: Utc::now(),
        tags: tags.clone(),
    });
    index.save(&repo)?;
    println!("staged note -{}", tags_suffix(&tags));
    Ok(())
}

/// `true` when the note text comes from stdin.
///
/// `-m` wins when stdin is a pipe, so a script can pass the text as an argument.
/// `-` forces stdin. A terminal with neither `-m` nor `-` has nothing to read.
fn resolve_note_input(
    message: Option<&str>,
    positional: Option<&str>,
    stdin_is_tty: bool,
) -> Result<bool> {
    match positional {
        None => {}
        Some("-") => {
            if message.is_some() {
                bail!("-m sets the note text; - reads stdin");
            }
            return Ok(true);
        }
        Some(other) => {
            bail!("{other} is not a note; stage files with mori add");
        }
    }
    if message.is_some() {
        return Ok(false);
    }
    if stdin_is_tty {
        bail!("a note needs -m, or text piped on stdin");
    }
    Ok(true)
}

fn note_preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(120).collect()
}

fn cmd_compile(
    sources: Vec<String>,
    kind: ContextKind,
    session: Option<String>,
    tags: &[String],
) -> Result<()> {
    let tags = normalize_tags(tags)?;
    let mut staged = if sources.is_empty() {
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
                    tags: tags.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?
    };
    if !tags.is_empty() {
        for entry in &mut staged {
            for tag in &tags {
                if !entry.tags.iter().any(|existing| existing == tag) {
                    entry.tags.push(tag.clone());
                }
            }
        }
    }

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
        println!(
            "  {} {}{}",
            entry.kind.as_str(),
            entry.source,
            tags_suffix(&entry.tags)
        );
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
        if !record.merge_parents.is_empty() {
            let mut parents = Vec::new();
            if let Some(parent) = &record.parent {
                parents.push(short(parent).to_string());
            }
            for parent in &record.merge_parents {
                parents.push(short(parent).to_string());
            }
            println!("Merge: {}", parents.join(" "));
        }
        println!("session {}", record.session);
        println!("Date: {}", record.created_at.to_rfc3339());
        println!();
        println!("    {}", record.message);
        println!();
        for entry in &record.entries {
            println!("    {} {}", entry.kind.as_str(), entry.source);
            println!("    {}", entry.summary);
            print_entry_tags(&entry.tags);
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
        print_entry_tags(&entry.tags);
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

async fn cmd_recall(
    query: &str,
    session: Option<&str>,
    tags: &[String],
    limit: usize,
    cut: &CutArgs,
    raw: bool,
) -> Result<()> {
    let tags = normalize_tags(tags)?;
    let pattern = MatchPattern::compile(cut.pattern.as_deref())?;
    let mode = display_mode(cut, raw)?;
    if matches!(mode, DisplayMode::Excerpt) && query.trim().is_empty() && pattern.is_none() {
        bail!("--excerpt needs a query or --match to choose sections");
    }
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let widen = !matches!(mode, DisplayMode::Summary) || pattern.is_some();
    let hits = memory.recall(query, session, &tags, limit, widen).await?;
    memory.disconnect();
    if matches!(mode, DisplayMode::Summary) && pattern.is_none() {
        print_recall_summaries(&hits, raw);
        return Ok(());
    }
    let viewed = view_hits(
        &hits,
        &ViewOptions {
            query: Some(query),
            pattern: pattern.as_ref(),
            literal: None,
            excerpt: matches!(mode, DisplayMode::Excerpt),
            full: matches!(mode, DisplayMode::Full),
            context_lines: cut.context_lines,
            limit,
        },
    )?;
    print_recall(&viewed, mode, raw);
    Ok(())
}

async fn cmd_find(
    session: Option<&str>,
    tags: &[String],
    contains: Option<&str>,
    limit: usize,
    cut: &CutArgs,
) -> Result<()> {
    let tags = normalize_tags(tags)?;
    let pattern = MatchPattern::compile(cut.pattern.as_deref())?;
    let mode = display_mode(cut, false)?;
    if matches!(mode, DisplayMode::Excerpt) && pattern.is_none() && contains.is_none() {
        bail!("--excerpt needs --match or --contains to choose sections");
    }
    let repo = repo_from_cwd()?;
    let memory = Memory::connect(&repo).await?;
    let widen = !matches!(mode, DisplayMode::Summary) || pattern.is_some();
    let hits = memory.find(session, contains, &tags, limit, widen).await?;
    memory.disconnect();
    if matches!(mode, DisplayMode::Summary) && pattern.is_none() {
        print_find_summaries(&hits);
        return Ok(());
    }
    let viewed = view_hits(
        &hits,
        &ViewOptions {
            query: None,
            pattern: pattern.as_ref(),
            literal: contains,
            excerpt: matches!(mode, DisplayMode::Excerpt),
            full: matches!(mode, DisplayMode::Full),
            context_lines: cut.context_lines,
            limit,
        },
    )?;
    print_find(&viewed, mode);
    Ok(())
}

fn display_mode(cut: &CutArgs, raw: bool) -> Result<DisplayMode> {
    if cut.excerpt && cut.full {
        bail!(
            "--excerpt prints matching sections and --full prints the stored text; choose one view"
        );
    }
    if raw && (cut.excerpt || cut.full) {
        bail!("--raw prints the stored STTP; --excerpt and --full print the document text");
    }
    if cut.context_lines.is_some() && !cut.excerpt {
        bail!("-C sets context lines for --excerpt");
    }
    Ok(if cut.full {
        DisplayMode::Full
    } else if cut.excerpt {
        DisplayMode::Excerpt
    } else {
        DisplayMode::Summary
    })
}

fn print_recall_summaries(hits: &[Recalled], raw: bool) {
    if hits.is_empty() {
        println!("nothing recalled");
        return;
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
}

fn print_recall(hits: &[ViewedHit], mode: DisplayMode, raw: bool) {
    if hits.is_empty() {
        println!("nothing recalled");
        return;
    }
    let path = hits
        .first()
        .map(|hit| hit.path.as_str())
        .unwrap_or("recall");
    println!("retrieved {} via {path}", hits.len());
    for (index, hit) in hits.iter().enumerate() {
        match mode {
            DisplayMode::Summary => {
                println!("{}", hit.summary);
                if raw {
                    println!("{}", hit.raw_sttp);
                }
            }
            DisplayMode::Excerpt | DisplayMode::Full => {
                if index > 0 {
                    println!();
                }
                for line in &hit.lines {
                    println!("{line}");
                }
            }
        }
    }
}

fn print_find_summaries(hits: &[Recalled]) {
    if hits.is_empty() {
        println!("nothing found");
        return;
    }
    println!("found {}", hits.len());
    for hit in hits {
        println!("{}", hit.summary);
    }
}

fn print_find(hits: &[ViewedHit], mode: DisplayMode) {
    if hits.is_empty() {
        println!("nothing found");
        return;
    }
    println!("found {}", hits.len());
    for (index, hit) in hits.iter().enumerate() {
        match mode {
            DisplayMode::Summary => println!("{}", hit.summary),
            DisplayMode::Excerpt | DisplayMode::Full => {
                if index > 0 {
                    println!();
                }
                for line in &hit.lines {
                    println!("{line}");
                }
            }
        }
    }
}

fn tags_suffix(tags: &[String]) -> String {
    if tags.is_empty() {
        String::new()
    } else {
        format!("  [{}]", tags.join(", "))
    }
}

fn print_entry_tags(tags: &[String]) {
    if !tags.is_empty() {
        println!("    tags: {}", tags.join(", "));
    }
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

async fn cmd_merge(branch: &str, message: Option<&str>) -> Result<()> {
    let repo = repo_from_cwd()?;
    refuse_dirty(&repo, "merging")?;
    let config = repo.load_config()?;
    let memory = Memory::connect(&repo).await?;
    let current = memory.current_branch().await?;
    let outcome = memory
        .merge(branch, message, &config.default_session)
        .await?;
    memory.disconnect();
    match outcome {
        MergeOutcome::UpToDate => println!("already up to date"),
        MergeOutcome::FastForward { commit_id } => {
            println!("fast-forward to {}", short(&commit_id));
        }
        MergeOutcome::Merged { commit } => {
            println!("[{current} {}] {}", short(&commit.id), commit.message);
        }
    }
    Ok(())
}

async fn cmd_rebase(onto: &str) -> Result<()> {
    let repo = repo_from_cwd()?;
    refuse_dirty(&repo, "rebasing")?;
    let memory = Memory::connect(&repo).await?;
    let outcome = memory.rebase(onto).await?;
    memory.disconnect();
    match outcome {
        RebaseOutcome::UpToDate => println!("already up to date"),
        RebaseOutcome::FastForward { commit_id } => {
            println!("fast-forward to {}", short(&commit_id));
        }
        RebaseOutcome::Rebased { commits, tip } => {
            println!("rebased {} onto {onto}", commits.len());
            for commit in &commits {
                println!("  {} {}", short(&commit.id), commit.message);
            }
            println!("HEAD {}", short(&tip));
        }
    }
    Ok(())
}

fn refuse_dirty(repo: &Repo, action: &str) -> Result<()> {
    if StageIndex::load(repo)?.is_empty() {
        Ok(())
    } else {
        bail!("staged context is in the way; stash or commit it before {action}")
    }
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

#[cfg(test)]
mod tests {
    use super::resolve_note_input;

    #[test]
    fn message_does_not_read_stdin() {
        assert!(!resolve_note_input(Some("hi"), None, false).unwrap());
        assert!(!resolve_note_input(Some("hi"), None, true).unwrap());
    }

    #[test]
    fn pipe_without_message_reads_stdin() {
        assert!(resolve_note_input(None, None, false).unwrap());
    }

    #[test]
    fn tty_without_message_errors() {
        let err = resolve_note_input(None, None, true).unwrap_err();
        assert!(err.to_string().contains("-m"), "{err}");
    }

    #[test]
    fn dash_reads_stdin_even_on_a_tty() {
        assert!(resolve_note_input(None, Some("-"), true).unwrap());
    }

    #[test]
    fn dash_and_message_conflict() {
        let err = resolve_note_input(Some("hi"), Some("-"), false).unwrap_err();
        assert!(err.to_string().contains("-m sets the note text"), "{err}");
    }

    #[test]
    fn file_path_is_rejected() {
        let err = resolve_note_input(None, Some("notes.md"), false).unwrap_err();
        assert!(err.to_string().contains("mori add"), "{err}");
    }
}
