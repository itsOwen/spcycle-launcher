#!/usr/bin/env bash
# build the appimage in ubuntu 22.04, so it runs on any distro. glibc symbols are
# versioned: one linked on a rolling distro starts there and nowhere older.
#
# the same script runs on both sides of the container. outside it sets up the
# mounts and re-enters; inside it does the build.
#
#     ./tools/build-appimage.sh [--sign]   # -> out/
set -euo pipefail
cd "$(dirname "$0")/.."

SIGNED=""
[ "${1:-}" = "--sign" ] && SIGNED=1

if [ "${SPCYCLE_IN_CONTAINER:-}" != 1 ]; then
    command -v docker >/dev/null && docker info >/dev/null 2>&1 || {
        echo "docker is not available, and building on the host would link against" >&2
        echo "this machine's glibc ($(ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+$'))," >&2
        echo "which will not start on an older distro." >&2
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
    echo "==> building the image (ubuntu 22.04)"
    docker build -f tools/Dockerfile.appimage -t spcycle-appimage tools/

    # the source is mounted, never copied, and the caches are named volumes, so a
    # second build reuses the crates and node_modules instead of refetching them.
    # CARGO_TARGET_DIR lands inside the mount on purpose: docker's own layer is on
    # / , which is far smaller than the tree's filesystem, and a rust build fills it.
    echo "==> building the appimage"
    docker run --rm "${SIGN[@]}" \
        -v "$PWD:/src" \
        -v spcycle-node-modules:/src/node_modules \
        -v spcycle-cargo-registry:/root/.cargo/registry \
        -v spcycle-cargo-git:/root/.cargo/git \
        -e npm_config_store_dir=/src/node_modules/.pnpm-store \
        -e CARGO_TARGET_DIR=/src/src-tauri/target/container \
        -e SPCYCLE_IN_CONTAINER=1 \
        -e CI=true \
        spcycle-appimage \
        bash -c "./tools/build-appimage.sh ${1:-}; rc=\$?; \
                 chown -R $(id -u):$(id -g) /src/src-tauri/target/container /src/dist /src/out 2>/dev/null; \
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
pnpm tauri build --bundles appimage

BUNDLE="${CARGO_TARGET_DIR:-src-tauri/target}/release/bundle/appimage"
mkdir -p out
found=$(find "$BUNDLE" -maxdepth 1 -name '*.AppImage*' -exec cp {} out/ \; -print | head -1)
[ -n "$found" ] || { echo "no AppImage was produced" >&2; exit 1; }
