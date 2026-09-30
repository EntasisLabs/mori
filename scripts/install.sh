#!/usr/bin/env bash
# Install a mori release binary from GitHub.
# Usage: curl -fsSL https://raw.githubusercontent.com/EntasisLabs/mori/main/scripts/install.sh | bash
set -euo pipefail

repo="${MORI_REPO:-EntasisLabs/mori}"
install_dir="${MORI_INSTALL_DIR:-${HOME}/.local/bin}"

die() {
  echo "mori: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

target_triple() {
  local os="$1" arch="$2"
  case "${os}:${arch}" in
    linux:x86_64 | linux:amd64) printf '%s\n' x86_64-unknown-linux-gnu ;;
    darwin:arm64 | darwin:aarch64) printf '%s\n' aarch64-apple-darwin ;;
    darwin:x86_64 | darwin:amd64) printf '%s\n' x86_64-apple-darwin ;;
    windows:x86_64 | windows:amd64) printf '%s\n' x86_64-pc-windows-msvc ;;
    *) return 1 ;;
  esac
}

detect_os() {
  local raw
  raw="$(uname -s | tr '[:upper:]' '[:lower:]')"
  case "${raw}" in
    linux) printf '%s\n' linux ;;
    darwin) printf '%s\n' darwin ;;
    mingw* | msys* | cygwin*) printf '%s\n' windows ;;
    *) printf '%s\n' "${raw}" ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) printf '%s\n' x86_64 ;;
    aarch64 | arm64) printf '%s\n' arm64 ;;
    *) uname -m ;;
  esac
}

resolve_tag() {
  local version="${MORI_VERSION:-}"
  if [[ -n "${version}" ]]; then
    if [[ "${version}" != v* ]]; then
      version="v${version}"
    fi
    printf '%s\n' "${version}"
    return
  fi

  local url tag
  url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/${repo}/releases/latest")" \
    || die "no GitHub release found for ${repo}. Push a v* tag, or set MORI_VERSION."
  tag="${url##*/}"
  if [[ ! "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.+-][0-9A-Za-z.+-]+)?$ ]]; then
    die "could not resolve the latest release (got '${tag}'). Set MORI_VERSION."
  fi
  printf '%s\n' "${tag}"
}

verify_sha256() {
  local file="$1" expected="$2"
  local got
  if command -v sha256sum >/dev/null 2>&1; then
    got="$(sha256sum "${file}" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    got="$(shasum -a 256 "${file}" | awk '{print $1}')"
  else
    die "missing sha256sum or shasum; cannot verify ${file}"
  fi
  if [[ "${got}" != "${expected}" ]]; then
    die "checksum mismatch for $(basename "${file}")"
  fi
}

extract_archive() {
  local archive="$1" dest="$2"
  mkdir -p "${dest}"
  case "${archive}" in
    *.tar.gz) tar -xzf "${archive}" -C "${dest}" ;;
    *.zip)
      if command -v unzip >/dev/null 2>&1; then
        unzip -q "${archive}" -d "${dest}"
      elif command -v python3 >/dev/null 2>&1; then
        python3 -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "${archive}" "${dest}"
      elif tar -xf "${archive}" -C "${dest}"; then
        :
      else
        die "missing unzip, python3, or a tar that can read zip files"
      fi
      ;;
    *) die "unknown archive type: ${archive}" ;;
  esac
}

if [[ "${MORI_SELF_TEST:-}" == 1 ]]; then
  fail=0
  expect() {
    local got=""
    if got="$(target_triple "$1" "$2")"; then
      :
    else
      got=""
    fi
    if [[ "${got}" != "$3" ]]; then
      echo "target_triple $1 $2: expected '$3', got '${got}'" >&2
      fail=1
    fi
  }
  expect linux x86_64 x86_64-unknown-linux-gnu
  expect linux amd64 x86_64-unknown-linux-gnu
  expect darwin arm64 aarch64-apple-darwin
  expect darwin aarch64 aarch64-apple-darwin
  expect darwin x86_64 x86_64-apple-darwin
  expect darwin amd64 x86_64-apple-darwin
  expect windows x86_64 x86_64-pc-windows-msvc
  expect windows amd64 x86_64-pc-windows-msvc
  expect linux arm64 ""
  expect linux aarch64 ""
  expect freebsd x86_64 ""
  exit "${fail}"
fi

need curl
need tar

os="${MORI_OS:-$(detect_os)}"
arch="${MORI_ARCH:-$(detect_arch)}"
if ! target="$(target_triple "${os}" "${arch}")"; then
  die "no release binary for ${os} ${arch}. Supported: linux x86_64, macOS arm64, macOS x86_64, Windows x86_64. See https://github.com/${repo}/releases"
fi

case "${target}" in
  *windows*) ext="zip" bin_name="mori.exe" ;;
  *) ext="tar.gz" bin_name="mori" ;;
esac

tag="$(resolve_tag)"
version="${tag#v}"
archive="mori-${version}-${target}.${ext}"
base="https://github.com/${repo}/releases/download/${tag}"

if [[ "${MORI_DRY_RUN:-}" == 1 ]]; then
  printf 'tag=%s\narchive=%s\nurl=%s\ninstall_dir=%s\n' "${tag}" "${archive}" "${base}/${archive}" "${install_dir}"
  exit 0
fi

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

echo "mori: downloading ${archive}"
if ! curl -fsSL --retry 3 --retry-delay 2 -o "${workdir}/${archive}" "${base}/${archive}"; then
  die "no release asset ${archive} for ${tag}. ${os} ${arch} needs that file on https://github.com/${repo}/releases"
fi
if ! curl -fsSL --retry 3 --retry-delay 2 -o "${workdir}/sha256sums.txt" "${base}/sha256sums.txt"; then
  die "release ${tag} has no sha256sums.txt"
fi

expected="$(awk -v file="${archive}" '$2 == file { print $1 }' "${workdir}/sha256sums.txt")"
if [[ -z "${expected}" ]]; then
  die "sha256sums.txt has no entry for ${archive}"
fi
verify_sha256 "${workdir}/${archive}" "${expected}"

extract_archive "${workdir}/${archive}" "${workdir}/extract"
bin_path=""
while IFS= read -r candidate; do
  bin_path="${candidate}"
  break
done < <(find "${workdir}/extract" -type f -name "${bin_name}")
if [[ -z "${bin_path}" ]]; then
  die "archive ${archive} does not contain ${bin_name}"
fi

mkdir -p "${install_dir}" || die "cannot create ${install_dir}"
if [[ ! -w "${install_dir}" ]]; then
  die "cannot write to ${install_dir}. Set MORI_INSTALL_DIR, or rerun with permission to write there (for /usr/local/bin: sudo env MORI_INSTALL_DIR=/usr/local/bin bash)."
fi

if command -v install >/dev/null 2>&1; then
  install -m 755 "${bin_path}" "${install_dir}/${bin_name}"
else
  cp "${bin_path}" "${install_dir}/${bin_name}"
  chmod 755 "${install_dir}/${bin_name}"
fi

echo "mori: installed ${install_dir}/${bin_name} (${tag})"
if [[ ":${PATH}:" != *":${install_dir}:"* ]]; then
  echo "mori: ${install_dir} is not on PATH"
  echo "  export PATH=\"${install_dir}:\$PATH\""
fi
