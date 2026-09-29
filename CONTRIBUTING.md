# Contributing

Thanks for looking. mori is a small command line tool, and the docs are part of the product. A change that works but reads like a spec is not done.

## License

mori is licensed under either of:

- MIT, see [LICENSE-MIT](LICENSE-MIT)
- Apache License, Version 2.0, see [LICENSE-APACHE](LICENSE-APACHE)

You may use it under either license. When you contribute, you agree that your contribution is available under both, the same way.

The prebuilt binary also links SurrealDB, which is under a different license. See [NOTICE](NOTICE). That notice is about redistribution of the binary, not about the license of your patch.

## Setup

You need Rust stable. The repo pins it in `rust-toolchain.toml`.

```bash
cargo test
cargo fmt --all --check
```

`cargo test` is the check that matters. The CLI tests start a real embedded database, so give them a moment.

## What to change

- Prefer the words a person would use. "Save", "branch", "ask", and "copy" beat storage jargon in help text and docs.
- Explain a term once, where it first appears. STTP, session, and fast-forward are the usual ones.
- A new command needs a CLI test in `tests/cli.rs` and a short mention in the README and in [skills/mori/SKILL.md](skills/mori/SKILL.md).
- Do not require the user to write STTP. Compile ordinary text, or accept STTP they already have.
- Open the database for one command and drop it. Do not add a daemon.

## Pull requests

Say what changed and how you checked it. A small patch is easier to review than a wide one. If you are unsure whether a behavior change is wanted, open an issue first.

## Conduct

Be direct and kind. Disagree with the change, not the person. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Security

See [SECURITY.md](SECURITY.md). Do not file a public issue for a vulnerability.
