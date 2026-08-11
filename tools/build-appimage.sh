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
APPDIR="$(find "$BUNDLE" -maxdepth 1 -type d -name '*.AppDir' -print -quit)"
APPIMAGE="$(find "$BUNDLE" -maxdepth 1 -name '*.AppImage' -print -quit)"
[ -n "$APPDIR" ] && [ -n "$APPIMAGE" ] || { echo "no AppImage was produced" >&2; exit 1; }

# linuxdeploy pulls the build machine's graphics stack into the bundle, and at run
# time those override the host's real drivers: "Could not create default EGL
# display: EGL_BAD_PARAMETER" and a blank window. the host always has its own.
removed="$(find "$APPDIR" \( -name 'libwayland-*' -o -name 'libEGL.so*' -o -name 'libGL.so*' \
                            -o -name 'libdrm.so*' -o -name 'libgbm.so*' -o -name 'libglapi.so*' \) \
                -print -delete)"
[ -n "$removed" ] && { echo "==> removed the bundled graphics stack:"; echo "$removed"; }

# linuxdeploy bakes a relative libexec path into webkit, which resolves against the
# cwd rather than the mount point, so the webview dies unless it is told the truth
wk="$(find "$APPDIR" -type d -name 'webkit2gtk-4.1' -print -quit 2>/dev/null || true)"
if [ -n "$wk" ]; then
    mkdir -p "$APPDIR/apprun-hooks"
    printf '#!/usr/bin/env bash\nexport WEBKIT_EXEC_PATH="${APPDIR}/%s"\n' "${wk#"$APPDIR"/}" \
        > "$APPDIR/apprun-hooks/zzz-spcycle-webkit.sh"
    grep -q 'zzz-spcycle-webkit.sh' "$APPDIR/AppRun" || sed -i \
        's|^exec "\$this_dir"/AppRun\.wrapped|source "$this_dir"/apprun-hooks/"zzz-spcycle-webkit.sh"\n\nexec "$this_dir"/AppRun.wrapped|' \
        "$APPDIR/AppRun"
    grep -q 'zzz-spcycle-webkit.sh' "$APPDIR/AppRun" \
        || { echo "could not add the webkit hook to AppRun; its layout changed" >&2; exit 1; }
fi

# the .AppImage tauri produced still holds what was just deleted, so rebuild it
TOOL="$(find "${CARGO_TARGET_DIR:-src-tauri/target}" "$HOME/.cache/tauri" -maxdepth 5 \
          -name 'linuxdeploy-plugin-appimage*.AppImage' -print -quit 2>/dev/null || true)"
[ -n "$TOOL" ] || { echo "the appimage plugin is missing, so the bundle still has the libraries" >&2; exit 1; }
chmod +x "$TOOL"
echo "==> repackaging"
( cd "$(dirname "$APPIMAGE")" \
  && APPIMAGE_EXTRACT_AND_RUN=1 OUTPUT="$(basename "$APPIMAGE")" \
     "$(readlink -f "$TOOL")" --appdir "$(readlink -f "$APPDIR")" )

# repackaging changed the bytes, so tauri's signature no longer matches them and
# every updater would reject it. sign what we are actually shipping.
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    echo "==> re-signing"
    rm -f "$APPIMAGE.sig"
    pnpm tauri signer sign \
        --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
        --password "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
        "$APPIMAGE"
    [ -f "$APPIMAGE.sig" ] || { echo "re-signing produced no .sig" >&2; exit 1; }
fi

mkdir -p out
find "$BUNDLE" -maxdepth 1 -name '*.AppImage*' -exec cp {} out/ \;
