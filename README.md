# mori

git for cognition

mori keeps local history for documents, chats, and notes. You stage files, commit them onto branches, and later look them up with log, recall, and find. STTP is the structured memory format Locus uses; mori compiles files into it so you don't write it by hand. The history is stored in a SurrealKV file inside the repo.

mori is the cognition VCS. [Locus](https://github.com/EntasisLabs/locus) is the engine.

## Status

Apache-2.0. 0.1.0 is a local CLI for now: no remote and no forge. Install from crates.io or GitHub Releases.

## Install

`cargo install mori` is the usual path. You need a recent stable Rust toolchain.

```bash
cargo install mori
```

The install script downloads the matching GitHub Release archive and puts `mori` in `~/.local/bin`.

```bash
curl -fsSL https://raw.githubusercontent.com/EntasisLabs/mori/main/scripts/install.sh | bash
```

`~/.local/bin` needs to be on `PATH`. Set `MORI_INSTALL_DIR` to choose another directory. `/usr/local/bin` is the other common choice:

```bash
curl -fsSL https://raw.githubusercontent.com/EntasisLabs/mori/main/scripts/install.sh | sudo env MORI_INSTALL_DIR=/usr/local/bin bash
```

`MORI_VERSION=v0.1.0` installs that tag instead of the latest release. The script supports Linux x86_64, macOS (Apple Silicon and Intel), and Windows x86_64 from Git Bash. The Linux archive is built on Ubuntu 24.04; on an older glibc, use `cargo install mori`.

To build from a clone:

```bash
git clone https://github.com/EntasisLabs/mori
cd mori
cargo install --path .
```

`cargo build --release` writes `target/release/mori` and does not install it. `rust-toolchain.toml` selects the stable channel.

There is no daemon. Each command opens the store, does one thing, and disconnects.

## Usage

```bash
mori init
mori add notes.md
mori add thread.json --kind chat
mori status
mori compile notes.md
mori commit -m "save the notes"
mori log
mori recall "notes"
mori find --contains notes
mori show <commit> --raw
```

`add` detects the kind from the file. Set `--kind` to `document`, `chat`, `note`, `context`, or `sttp` to override detection. `auto` is the default. Pass `-` to read stdin.

`compile` prints STTP and does not store it. With no paths, it compiles whatever is staged.

`commit` requires `-m`. `log` lists commits on the current branch, newest first, 20 by default (`-n` changes the limit). `show` takes a commit id or a unique prefix.

`recall` ranks stored context against the query (8 hits by default). `find` filters without ranking. `log`, `status`, `recall`, and `find` print the summary. The STTP text stays in the store until you ask for it with `compile`, `show --raw`, or `recall --raw`.

## Branches

A branch is a name pointing at a commit. `log`, `recall`, and `find` follow every parent from that commit. A merge makes both sides visible. Context committed only on another branch stays out of view. Shared history stays visible on both. Nodes are not copied.

```bash
mori branch
mori branch design
mori checkout -b design
mori checkout main
```

`branch` with no name lists branches and marks the current one with `*`. `branch <name>` creates a branch at the current tip. `checkout -b` creates that branch and switches to it. `checkout` refuses when context is staged and you are switching branches.

`stash` parks the index so you can check out another branch. `stash pop` restores the newest entry onto an empty index. `stash drop` discards the newest entry.

```bash
mori stash -m "hold the design notes"
mori stash list
mori stash pop
mori stash drop
```

`reset` unstages. With no paths it clears the index and leaves stored memory and the stash alone.

```bash
mori reset
mori reset notes.md
```

`merge` brings another branch into the current one. If the current tip already contains the other branch, mori leaves the pointer where it is. If the other branch contains the current tip, mori moves the pointer forward. Otherwise it writes a merge commit with both parents and no new context. Stored context is additive, so both sides stay visible and there is no content-conflict step.

`rebase` replays the commits that belong only to the current branch onto another tip. The branch you rebase onto stays where it is. `rebase` refuses a merge commit, because replaying one would drop a parent.

`merge` and `rebase` refuse when context is staged. This history is local to the repo.

```bash
mori merge design
mori merge design -m "bring the design notes back"
mori rebase main
```

## Sessions

`--session` is a label Locus stores on the context. `checkout` is what changes the branch. The default session is `main`.

```bash
mori init --session design
mori add design.md --session design
mori recall "layout" --session design
mori find --session design
```

`init --session` sets the repo default. `add --session` labels the files in that command. `recall` and `find` take `--session` to limit the search to that label.

## Layout

```text
.mori/
  config.json    repo settings, including the default session
  index.json     staged context that is not stored yet
  stash.json     parked index entries, written on the first stash
  kv/            embedded SurrealKV directory
```

`add` and `compile` never open `kv/`. Commands that read or write memory open it and disconnect before they return.

## What gets stored

| Input | What mori does |
| --- | --- |
| document, note, context | Locus `build_content_from_text`, then a canonical STTP document |
| chat (JSON messages, or `user:` / `assistant:` lines) | same compiler, with each turn nested under a conversation |
| raw STTP | validated and parsed, then stored as-is |

## License

Apache-2.0. See [LICENSE](LICENSE).
