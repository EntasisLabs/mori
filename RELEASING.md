# Releasing

A release is a git tag. The release workflow builds the binaries, writes `checksums.txt`, and attaches both to a GitHub release. It does not run until the tag is pushed.

## Tag

The version in `Cargo.toml` should match the tag, without the leading `v`.

```bash
# Cargo.toml says version = "0.1.0"
git tag v0.1.0
git push origin v0.1.0
```

Push the tag. Do not create the GitHub release by hand first. The workflow creates it.

The workflow builds:

| Machine | Archive |
| --- | --- |
| Linux x86_64 | `mori-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `mori-aarch64-unknown-linux-gnu.tar.gz` |
| macOS arm64 | `mori-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `mori-x86_64-pc-windows-msvc.tar.gz` |

Each archive contains the `mori` binary (`mori.exe` on Windows). `checksums.txt` has one SHA-256 line per archive, in `sha256sum` form: hash, two spaces, filename.

Intel Macs are not in the matrix. Build those from source with `cargo install --locked`.

## Check

After the workflow finishes, download `checksums.txt` and one archive and run `sha256sum -c checksums.txt --ignore-missing`. `install.sh` does this before it copies the binary.

## crates.io

`publish` is `false` in `Cargo.toml`. The source is ready to dual-license, and the checked install path is the GitHub release. Turn `publish` on only when a maintainer intends to ship the crate, then `cargo publish --dry-run` before `cargo publish`.

## Binary license

The source of mori is MIT OR Apache-2.0. The binary links SurrealDB 3, which is Business Source License 1.1 until 2030-01-01. [NOTICE](NOTICE) has to stay with anything you redistribute.
