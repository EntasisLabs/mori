# mori

the git for cognition.

mori takes context — a document, a chat, a note, or raw STTP — and either compiles it with the [Locus](https://github.com/EntasisLabs/locus) SDK or stores it in a SurrealKV file that belongs to that repo. You do not write STTP by hand unless you already have it.

The database is the official Locus path: `locus-surreal-adapter` opens a `surrealkv://` file, and `SurrealDbNodeStore` is the memory. Each command connects, does one thing, and drops the client. There is no daemon.

## Layout

```text
.mori/
  config.json    repo settings, including the default session
  index.json     staged context that is not stored yet
  kv/            embedded SurrealKV directory
```

`add` and `compile` never open `kv/`. Commands that read or write memory open it and disconnect before they return.

## Usage

```bash
mori init
mori add notes.md                  # kind is detected
mori add thread.json --kind chat
mori status
mori compile notes.md              # print STTP, do not store it
mori commit -m "remember the orchard"
mori log
mori recall "orchard plan"
mori show <commit> --raw           # raw STTP is opt-in
```

A branch is a name pointing at a commit. `log`, `recall`, and `find` follow every parent from that commit, so a merge makes both sides visible and context committed only on another branch stays out of view. Shared history stays visible on both. Nodes are not copied.

```bash
mori branch                        # list, current branch marked *
mori checkout -b kiln              # new branch at the current tip
mori checkout main                 # refuses if context is staged
```

`stash` parks the index so you can check out another branch. `stash pop` puts it back onto an empty index.

```bash
mori stash -m "hold the kiln notes"
mori stash list
mori stash pop
```

`merge` brings another branch into the current one. When one tip already contains the other, mori moves the branch pointer. Otherwise it writes a merge commit with both parents and no new context. Stored context is additive, so both sides stay visible and there is no content-conflict step.

`rebase` replays the commits that belong only to the current branch onto another tip. The branch you rebase onto stays where it is. A merge commit is refused rather than replayed with a parent dropped.

```bash
mori merge kiln
mori merge kiln -m "bring the kiln notes back"
mori rebase main
```

Both refuse when context is staged. This history is local to the repo.

`--session` is a label Locus stores on the context. It is not a branch. The default session is `main`.

```bash
mori add design.md --session design
```

`reset` unstages. With no paths it clears the index and leaves stored memory and the stash alone.

## What gets stored

| Input | What mori does |
| --- | --- |
| document, note, context | Locus `build_content_from_text`, then a canonical STTP document |
| chat (JSON messages, or `user:` / `assistant:` lines) | same compiler, with each turn nested under a conversation |
| raw STTP | validated and parsed, then stored as-is |

`log`, `status`, `recall`, and `find` print the human summary. The STTP wire text stays in the store until you ask for it with `compile` or `show --raw`.
