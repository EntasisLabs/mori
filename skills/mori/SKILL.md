---
name: mori
description: Use the mori CLI to remember documents, chats, and notes, then recall them later. Use when the user mentions mori, a cognition repo, .mori, STTP notes, or asks to save, recall, branch, merge, stash, fetch, push, or sync context.
---

# mori

mori is a local command line tool. It remembers context the way git remembers code. There is no daemon. Each command opens the memory file, does one thing, and exits.

Run commands in the project directory. A mori folder contains `.mori/`. If that directory is missing, run `mori init` before any command that reads or writes memory. `compile` is the exception: it does not need a folder and does not open the database.

Do not write STTP by hand for the user. Give mori the document, chat, or note. Do not print raw STTP unless the user asks for the stored format (`compile` or `show --raw`).

## First save

```bash
mori init
mori add notes.md
mori status
mori commit -m "remember the orchard"
mori recall "orchard plan"
```

`add` stages. `commit` saves and clears the stage. `recall` asks the current branch. An empty answer is the line `nothing recalled`. `find --contains word` lists matches, or prints `nothing found`.

`mori add -` reads stdin. `--kind` is `auto` by default. Use `--kind chat` for a transcript, `--kind sttp` only when the file is already STTP.

## What to tell the user

`log`, `status`, `recall`, and `find` already print a short summary. Quote that. Do not dump `.mori/kv`.

`status` prints the session label, the branch (`On branch …`), `HEAD`, a stash count when one exists, and either `nothing staged` or the staged files.

## Branches

A branch is a line of history. `--session` is a label stored on the context. It is not a branch. Do not use `--session` to switch history.

```bash
mori branch
mori checkout -b kiln
mori checkout main
```

`checkout` to a different branch fails while context is staged. The error contains `staged context is in the way`. Then either:

- `mori commit -m "…"`, or
- `mori stash -m "why"`, switch, and later `mori stash pop`, or
- `mori reset` to unstage. Reset does not delete saved memory or the stash.

`stash pop` fails if something is already staged. `stash drop` discards the newest stash. `stash list` shows entries, newest first.

`log`, `recall`, and `find` follow every parent of the current branch. Notes that exist only on another branch stay hidden. Shared history stays visible.

## Merge and rebase

```bash
mori merge kiln
mori merge kiln -m "bring the kiln notes back"
mori rebase main
```

Both refuse a dirty stage. Merging a branch into itself is an error. The message contains `itself`.

`merge` fast-forwards when one tip already contains the other (`fast-forward to …`). Otherwise it writes one merge commit and no new context (`[branch id] message`). A second merge of the same tip prints `already up to date`.

`rebase` replays this branch's own commits onto the named branch and does not move that branch. It refuses a merge commit. The message contains `merge commits`. After a rebase, the other branch still does not recall the replayed notes until a later merge.

## Remotes

A remote is another SurrealDB. Accept `ws://`, `wss://`, `http://`, `https://`, or `surrealkv://` to another repo's `.mori/kv`.

```bash
mori remote add origin wss://memory.example/rpc
mori fetch
mori merge origin/main
mori push
mori sync
```

`fetch` updates `origin/<branch>` and does not move the branch you have checked out. Recall on the current branch will not see the fetched notes until you merge that tracking branch or fast-forward onto it.

`push` sends the current branch only. Check out a local branch first. A branch name containing `/` is a tracking branch. Pushing it fails with `check out a local branch before pushing`.

`push` refuses when the remote has diverged. The message contains `diverged`. Fetch, `mori merge origin/<branch>`, then push.

`sync` fetches, fast-forwards the current branch when the remote is strictly ahead, then pushes. If the histories diverged, it does not push. The error tells you to merge.

Do not put a remote password on the command line if you can avoid it. `--password` is stored in `.mori/config.json`. Do not commit that file, and do not echo the password.

## Output you can trust

- Success is exit code 0. Failure prints `mori: …` on stderr and exits 1.
- `nothing recalled` and `nothing found` are successful empty results.
- Commit lines look like `[main abcdef123456] message`.
- `show <prefix>` accepts a unique commit id prefix. `--raw` prints STTP.

## Do not

- Do not start a long-running process. mori is not a server.
- Do not edit files inside `.mori/kv`.
- Do not invent a sync protocol. Use `fetch`, `push`, and `sync`.
- Do not treat `--session` as `checkout`.
- Do not show raw STTP in a summary the user did not ask for.
