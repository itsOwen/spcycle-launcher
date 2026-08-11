#!/usr/bin/env bash
# type-check the #[cfg(windows)] half of the backend from linux, which a normal
# cargo build here never compiles a line of.
#
# two levels: the cryptoapi module alone against the windows target (pure rust,
# needs only a rustup target), and the whole crate when a mingw cross compiler
# is present, because aws-lc-sys has a c build script.
#
# neither replaces building on a real windows machine.
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=x86_64-pc-windows-gnu
MODULE=src-tauri/src/cert/windows.rs

if ! rustup target list --installed | grep -qx "$TARGET"; then
    echo "==> adding the $TARGET std library"
    rustup target add "$TARGET"
fi

echo "==> level 1: $MODULE in isolation"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/src"

# the dependency list is lifted out of the real manifest rather than written
# here. hardcoding it once meant this check passed against a windows-sys the
# crate itself did not declare, which is exactly the failure it exists to catch.
{
    cat <<'EOF'
[package]
name = "wincheck"
version = "0.0.0"
edition = "2021"

[dependencies]
thiserror = "2"
EOF
    python3 - src-tauri/Cargo.toml <<'EOF'
import json, sys, tomllib

def toml(v):
    if isinstance(v, str):
        return json.dumps(v)
    if isinstance(v, list):
        return "[" + ", ".join(toml(x) for x in v) + "]"
    if isinstance(v, dict):
        return "{ " + ", ".join(f"{k} = {toml(x)}" for k, x in v.items()) + " }"
    return json.dumps(v)

with open(sys.argv[1], "rb") as f:
    deps = tomllib.load(f)["target"]["cfg(windows)"]["dependencies"]
for name, spec in deps.items():
    print(f"{name} = {toml(spec)}")
EOF
} > "$WORK/Cargo.toml"

cat > "$WORK/src/lib.rs" <<'EOF'
#[derive(Debug, thiserror::Error)]
pub enum CertError {
    #[error("{0}")]
    StoreFailed(String),
}
pub mod windows_store;
EOF

sed 's|use super::CertError;|use crate::CertError;|' "$MODULE" > "$WORK/src/windows_store.rs"

( cd "$WORK" && cargo check --all-targets --target "$TARGET" )
echo "    ok"

echo "==> level 2: the whole crate"
if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    cargo check --manifest-path src-tauri/Cargo.toml --all-targets --target "$TARGET"
    echo "    ok"
elif command -v docker >/dev/null 2>&1; then
    # borrow a toolchain rather than ask for one on the host. the cargo cache is a
    # named volume so a re-run is quick.
    echo "    no host mingw; using docker"
    # rust-version in Cargo.toml is our floor, not the lockfile's: several
    # transitive crates need much newer, so track the host toolchain instead.
    RUSTC_IMAGE="rust:$(rustc --version | cut -d' ' -f2)"
    docker run --rm \
        -v "$PWD:/w" -w /w \
        -v spcycle-wincheck-cargo:/usr/local/cargo/registry \
        -v spcycle-wincheck-target:/w/src-tauri/target \
        "$RUSTC_IMAGE" bash -c "
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq gcc-mingw-w64-x86-64 >/dev/null
rustup target add $TARGET >/dev/null
mkdir -p dist && touch dist/index.html
cargo check --manifest-path src-tauri/Cargo.toml --all-targets --target $TARGET
"
    echo "    ok"
else
    cat <<'EOF'
    skipped: no x86_64-w64-mingw32-gcc on PATH and no docker.

    `aws-lc-sys` (pulled in transitively by steamroom's reqwest) builds C code,
    so a full cross-check needs a MinGW toolchain:

        Arch:   sudo pacman -S mingw-w64-gcc
        Debian: sudo apt install gcc-mingw-w64-x86-64

    Until then a real Windows build is the gate for everything except the
    module checked at level 1.
EOF
fi
