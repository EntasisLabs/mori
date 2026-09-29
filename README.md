# mori

the git for cognition.

mori remembers documents, chats, and notes in a folder. You save them, you ask questions later, and you can keep more than one line of memory. You do not have to learn a special file format. If you already have one, mori will take that too.

It feels like git, but the thing you save is context, not code.

## Install

Prebuilt binaries are published on [GitHub releases](https://github.com/EntasisLabs/mori/releases). Each release includes `checksums.txt`. The install script checks the archive against that file before it copies the binary.

Read the script, then run it:

```bash
curl -fsSL -o install.sh https://raw.githubusercontent.com/EntasisLabs/mori/main/install.sh
sh install.sh
```

A specific version:

```bash
MORI_VERSION=v0.1.0 sh install.sh
```

The binary lands in `~/.local/bin`. If that directory is not on your `PATH`, add it, or set `MORI_INSTALL_DIR`.

To check a download yourself:

```bash
curl -fsSLO https://github.com/EntasisLabs/mori/releases/download/v0.1.0/checksums.txt
curl -fsSLO https://github.com/EntasisLabs/mori/releases/download/v0.1.0/mori-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c checksums.txt --ignore-missing
```

On a Mac, `shasum -a 256 -c` does the same job. Pick the archive that matches your machine. The names are listed in `checksums.txt`.

From source, with Rust stable:

```bash
cargo install --git https://github.com/EntasisLabs/mori --locked
```

`--locked` builds the exact dependency set in `Cargo.lock`.

There is no background process. Each command opens the memory file, does one thing, and exits.

## Start here

```bash
mkdir orchard && cd orchard
mori init
```

Write a note, then save it:

```bash
printf 'the orchard plan waits on the north fence\n' > notes.md
mori add notes.md
mori status
mori commit -m "remember the orchard"
mori recall "orchard plan"
```

`add` sets the file aside to be saved. `commit` saves it. `recall` asks the current line of memory a question. `status` shows where you are and what is waiting to be saved.

`mori --help` lists every command. `mori commit --help` explains one of them.

## Everyday commands

| Command | What it does |
| --- | --- |
| `mori init` | Start a mori folder here |
| `mori add <file>` | Stage a file. Kind is detected |
| `mori status` | Show the branch, what is staged, and any stash |
| `mori commit -m "message"` | Save what is staged |
| `mori log` | Show saved history, newest first |
| `mori recall "question"` | Ask what this branch remembers |
| `mori find --contains word` | List matches without ranking them |
| `mori show <commit>` | Show one save. Add `--raw` for the stored format |
| `mori compile <file>` | Preview the stored format. Nothing is saved |
| `mori reset` | Unstage. Stored memory stays |

`log`, `status`, `recall`, and `find` print a short summary a person can read. The underlying format stays in the file until you ask for it with `compile` or `show --raw`.

## What you can give it

| You have | What to do |
| --- | --- |
| A document, note, or other prose | `mori add notes.md` |
| A chat, as JSON messages or `user:` / `assistant:` lines | `mori add thread.json` or `mori add thread.txt --kind chat` |
| Text piped from another program | `some-command \| mori add -` |
| The raw storage format already | `mori add page.sttp --kind sttp` |

`--kind` can be `auto`, `document`, `chat`, `note`, `context`, or `sttp`. `auto` is the default. See [The format underneath](#the-format-underneath) if you want to know what `sttp` means. You can skip that section and still use mori.

## Branches

A branch is a named line of memory. `main` is the one you start on. `log`, `recall`, and `find` follow that line only, so notes saved on another branch stay out of view. History you already shared stays visible on both. The notes themselves are not copied.

```bash
mori branch                 # list branches. * marks the current one
mori checkout -b kiln       # start kiln at the current save, and switch to it
mori checkout main          # switch back
```

Switching is refused while something is staged. Save it, unstage it, or set it aside with stash.

```bash
mori stash -m "hold the kiln notes"
mori stash list
mori stash pop              # puts it back, only if nothing is staged
mori stash drop             # throws away the newest stashed page
```

`merge` brings another branch into the one you are on.

- If one side already contains the other, mori moves the pointer forward. That move is a fast-forward.
- If both sides saved different things, mori writes a merge save with both parents. Nothing new is stored. Both sides become visible, because saved context adds. There is no conflict step.

```bash
mori merge kiln
mori merge kiln -m "bring the kiln notes back"
```

`rebase` replays the saves that belong only to your current branch on top of another branch. The other branch stays where it is. A merge save is refused, because replaying it would drop one parent.

```bash
mori rebase main
```

`merge` and `rebase` also refuse while something is staged.

## Share a copy

A remote is another SurrealDB database. The usual addresses start with `ws://`, `wss://`, `http://`, or `https://`. A `surrealkv://` path to another folder's `.mori/kv` works the same way, which is useful on one machine.

```bash
mori remote add origin wss://memory.example/rpc
mori remote
```

Optional flags: `--user`, `--password`, `--namespace`, `--database`. The namespace defaults to `mori` and the database to `memory`. A password is written into `.mori/config.json`. Do not commit that file if it has one.

```bash
mori fetch                  # download. origin/main updates. your branch stays put
mori merge origin/main      # bring that line into the branch you are on
mori push                   # send the current branch
mori sync                   # fetch, fast-forward if you are simply behind, then push
```

`push` refuses when the two copies have diverged. Fetch, merge `origin/main`, then push. `sync` stops in that same situation and tells you to merge.

## Labels

`--session` is a label stored on the context, like a folder name inside the memory. It is not a branch. The default label is `main`.

```bash
mori add design.md --session design
mori recall "question" --session design
```

## Where files live

```text
.mori/
  config.json    settings, the default label, remotes, and a repo id
  index.json     context that is staged and not saved yet
  stash.json     context you set aside
  kv/            the memory file
```

`add` and `compile` do not open `kv/`. Commands that read or write memory open it and close it before they return.

Treat `.mori/` as private. `kv/` is the memory. `config.json` can hold a remote password.

## The format underneath

mori stores context with [Locus](https://github.com/EntasisLabs/locus). Locus keeps each piece as STTP, a small structured document. mori compiles ordinary text into that document for you.

`compile` prints the document and does not save it. `show <commit> --raw` prints what was saved. You only need this when you want to see the wire format, or when you already have STTP and want to store it unchanged.

The memory file is an embedded SurrealKV database, opened through the official Locus adapter. There is no separate server to run for a local folder.

## For agents

If you are an agent working in this repo, read [skills/mori/SKILL.md](skills/mori/SKILL.md) before you use the command. [AGENTS.md](AGENTS.md) points at the same file.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The source is licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. The prebuilt binary also includes SurrealDB. Read [NOTICE](NOTICE) before you redistribute a binary.
