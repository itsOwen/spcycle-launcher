#!/usr/bin/env bash
# build the windows installer from linux, so a release needs one machine.
# cargo-xwin supplies the MSVC CRT and Windows SDK; NSIS does the bundling.
#
# the same script runs on both sides of the container. outside it sets up the
# mounts and re-enters; inside it does the build.
#
#     ./tools/build-windows.sh [--sign]   # -> out/
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

    # separate target dir from the appimage build: same tree, different triple,
    # and sharing one would make the two builds evict each other's artifacts
    echo "==> building the installer"
    docker run --rm "${SIGN[@]}" \
        -v "$PWD:/src" \
        -v spcycle-node-modules:/src/node_modules \
        -v spcycle-cargo-registry:/root/.cargo/registry \
        -v spcycle-cargo-git:/root/.cargo/git \
        -v spcycle-xwin:/root/.cache/cargo-xwin \
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

# --bundles is deliberately not passed: the cli validates that list against the
# host, so `--bundles nsis` is rejected on linux, while the target alone picks
# the windows bundles correctly.
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc

BUNDLE="${CARGO_TARGET_DIR:-src-tauri/target}/x86_64-pc-windows-msvc/release/bundle"
mkdir -p out
found=$(find "$BUNDLE" -name '*-setup.exe*' -exec cp {} out/ \; -print | head -1)
[ -n "$found" ] || { echo "no installer was produced" >&2; exit 1; }
