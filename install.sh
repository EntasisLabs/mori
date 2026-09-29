#!/bin/sh
# Install a released mori binary after checking its SHA-256.
#
#   sh install.sh
#   MORI_VERSION=v0.1.0 sh install.sh
#   MORI_INSTALL_DIR=/usr/local/bin sh install.sh
#
# The script downloads checksums.txt and the archive for this machine from
# GitHub, refuses to continue if the hash does not match, then copies the
# binary into MORI_INSTALL_DIR (default: ~/.local/bin).

set -eu

REPO="${MORI_REPO:-EntasisLabs/mori}"
VERSION="${MORI_VERSION:-}"
PREFIX="${MORI_INSTALL_DIR:-$HOME/.local/bin}"

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    cat <<'EOF'
Install mori and check the release checksum.

  sh install.sh
  MORI_VERSION=v0.1.0 sh install.sh
  MORI_INSTALL_DIR=/usr/local/bin sh install.sh

Downloads checksums.txt and the archive for this machine, checks SHA-256,
then copies the binary into MORI_INSTALL_DIR (default ~/.local/bin).
EOF
    exit 0
fi

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "mori install: need '$1' on PATH" >&2
        exit 1
    fi
}

need curl

os=$(uname -s)
arch=$(uname -m)
target=""
bin="mori"

case "$os:$arch" in
    Linux:x86_64 | Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
    Linux:aarch64 | Linux:arm64) target="aarch64-unknown-linux-gnu" ;;
    Darwin:arm64) target="aarch64-apple-darwin" ;;
    MINGW*:x86_64 | MSYS*:x86_64 | CYGWIN*:x86_64)
        target="x86_64-pc-windows-msvc"
        bin="mori.exe"
        ;;
    *)
        echo "mori install: no prebuilt binary for $os $arch." >&2
        echo "Build from source: cargo install --git https://github.com/${REPO} --locked" >&2
        exit 1
        ;;
esac

if [ -n "${MORI_TARGET:-}" ]; then
    target=$MORI_TARGET
fi

if [ -z "$VERSION" ]; then
    VERSION=$(
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" |
            sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' |
            head -n 1
    )
fi

if [ -z "$VERSION" ]; then
    echo "mori install: no release found. Set MORI_VERSION=vX.Y.Z" >&2
    exit 1
fi

archive="mori-${target}.tar.gz"
base="https://github.com/${REPO}/releases/download/${VERSION}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading ${VERSION} ${archive}"
curl -fsSL "${base}/checksums.txt" -o "${tmp}/checksums.txt"
curl -fsSL "${base}/${archive}" -o "${tmp}/${archive}"

line=$(grep -F "${archive}" "${tmp}/checksums.txt" || true)
if [ -z "$line" ]; then
    echo "mori install: checksums.txt has no line for ${archive}" >&2
    exit 1
fi
printf '%s\n' "$line" >"${tmp}/check.txt"

(
    cd "$tmp"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c check.txt
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c check.txt
    else
        echo "mori install: need sha256sum or shasum" >&2
        exit 1
    fi
)

mkdir -p "${tmp}/out"
tar -xzf "${tmp}/${archive}" -C "${tmp}/out"

if [ ! -f "${tmp}/out/${bin}" ]; then
    echo "mori install: archive did not contain ${bin}" >&2
    exit 1
fi

mkdir -p "$PREFIX"
if command -v install >/dev/null 2>&1; then
    install -m 0755 "${tmp}/out/${bin}" "${PREFIX}/${bin}"
else
    cp "${tmp}/out/${bin}" "${PREFIX}/${bin}"
    chmod 0755 "${PREFIX}/${bin}"
fi

echo "installed ${PREFIX}/${bin}"
echo "checked ${VERSION} against checksums.txt"

case ":$PATH:" in
    *":${PREFIX}:"*) ;;
    *)
        echo "mori install: ${PREFIX} is not on PATH" >&2
        ;;
esac
