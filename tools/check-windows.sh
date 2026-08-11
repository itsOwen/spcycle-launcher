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

cat > "$WORK/Cargo.toml" <<EOF
[package]
name = "wincheck"
version = "0.0.0"
edition = "2021"

[dependencies]
windows-sys = { version = "0.59", features = ["Win32_Foundation", "Win32_Security_Cryptography"] }
thiserror = "2"
EOF

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
else
    cat <<'EOF'
    skipped: no x86_64-w64-mingw32-gcc on PATH.

    `aws-lc-sys` (pulled in transitively by steamroom's reqwest) builds C code,
    so a full cross-check needs a MinGW toolchain:

        Arch:   sudo pacman -S mingw-w64-gcc
        Debian: sudo apt install gcc-mingw-w64-x86-64

    Until then a real Windows build is the gate for everything except the
    module checked at level 1.
EOF
fi
