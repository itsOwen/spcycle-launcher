#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

SIGNED=""
[ "${1:-}" = "--sign" ] && SIGNED=1

if [ "${SPCYCLE_IN_CONTAINER:-}" != 1 ]; then
    command -v docker >/dev/null && docker info >/dev/null 2>&1 || {
        echo "docker is not available; the windows installer is cross-built inside it." >&2
        exit 1
    }

    SIGN=()
    if [ -n "$SIGNED" ]; then
        [ -f .secrets/spcycle.key ] || { echo "no signing key at .secrets/spcycle.key" >&2; exit 1; }
        SIGN=(
            -e "TAURI_SIGNING_PRIVATE_KEY=$(cat .secrets/spcycle.key)"
            -e "TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$(cat .secrets/spcycle.key.password)"
        )
    fi

    mkdir -p out
    echo "==> building the image (ubuntu 22.04, cargo-xwin + nsis)"
    docker build -f tools/Dockerfile.windows -t spcycle-windows tools/

    echo "==> building the installer"
    docker run --rm "${SIGN[@]}" \
        -v "$PWD:/src" \
        -v spcycle-node-modules:/src/node_modules \
        -v spcycle-cargo-registry:/root/.cargo/registry \
        -v spcycle-cargo-git:/root/.cargo/git \
        -v spcycle-xwin:/root/.cache/cargo-xwin \
        -v spcycle-tauri-cache:/root/.cache/tauri \
        -e npm_config_store_dir=/src/node_modules/.pnpm-store \
        -e CARGO_TARGET_DIR=/src/src-tauri/target/container-windows \
        -e SPCYCLE_IN_CONTAINER=1 \
        -e CI=true \
        spcycle-windows \
        bash -c "./tools/build-windows.sh ${1:-}; rc=\$?; \
                 chown -R $(id -u):$(id -g) /src/src-tauri/target/container-windows /src/dist /src/out 2>/dev/null; \
                 exit \$rc"
    echo
    ls -la out/
    exit 0
fi

# ---- inside the container ----

[ -f src-tauri/resources/depot.blob ] || {
    echo "src-tauri/resources/depot.blob is missing. run ./tools/fetch-depot-blob.sh on the host." >&2
    exit 1
}

pnpm install --frozen-lockfile

pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc

BUNDLE="${CARGO_TARGET_DIR:-src-tauri/target}/x86_64-pc-windows-msvc/release/bundle"
mkdir -p out

VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
find "$BUNDLE" -name "*_${VERSION}_*-setup.exe*" -exec cp {} out/ \;
[ -f "out/SPCycle Launcher_${VERSION}_x64-setup.exe" ] || { echo "no installer for $VERSION" >&2; exit 1; }
[ -f "out/SPCycle Launcher_${VERSION}_x64-setup.exe.sig" ] || { echo "installer for $VERSION has no .sig" >&2; exit 1; }
