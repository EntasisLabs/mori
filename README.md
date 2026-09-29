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

`add` and `compile` never open `kv/`. `init`, `commit`, `log`, `show`, `status`, `recall`, and `find` open it and disconnect before they return.

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

Sessions are the scope Locus stores nodes under. The default session is `main`.

```bash
mori add design.md --session design
```

`reset` unstages. With no paths it clears the index and leaves stored memory alone.

## What gets stored

| Input | What mori does |
| --- | --- |
| document, note, context | Locus `build_content_from_text`, then a canonical STTP document |
| chat (JSON messages, or `user:` / `assistant:` lines) | same compiler, with each turn nested under a conversation |
| raw STTP | validated and parsed, then stored as-is |

`log`, `status`, `recall`, and `find` print the human summary. The STTP wire text stays in the store until you ask for it with `compile` or `show --raw`.
