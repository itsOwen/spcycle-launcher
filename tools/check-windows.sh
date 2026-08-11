#!/usr/bin/env bash
# type-check the #[cfg(windows)] half from linux, which a normal build never compiles.
# level 1 is the cryptoapi module (rustup target only), level 2 the whole crate (needs
# mingw, for aws-lc-sys). neither replaces building on real windows.
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

# lifted out of the real manifest: hardcoding it once let this pass against a
# windows-sys the crate never declared, the exact failure it exists to catch
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
