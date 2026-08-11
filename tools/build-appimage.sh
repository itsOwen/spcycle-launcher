#!/usr/bin/env bash
# build the appimage inside ubuntu 22.04 so it runs on any distro.
#
# building on the host links against the host's glibc, and glibc symbols are
# versioned: an appimage built on arch will not start on ubuntu or debian. it
# also keeps linuxdeploy off a distro it does not get on with.
#
#     ./tools/build-appimage.sh          # -> out/
#     ./tools/build-appimage.sh --sign   # signed, for a release
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

command -v docker >/dev/null || { echo "docker is not installed" >&2; exit 1; }

[ -f src-tauri/resources/depot.blob ] || {
    echo "src-tauri/resources/depot.blob is missing. run ./tools/fetch-depot-blob.sh first." >&2
    exit 1
}

SIGN=()
if [ "${1:-}" = "--sign" ]; then
    KEY="$ROOT/.secrets/spcycle.key"
    [ -f "$KEY" ] || { echo "no signing key at $KEY" >&2; exit 1; }
    SIGN=(
        -e "TAURI_SIGNING_PRIVATE_KEY=$(cat "$KEY")"
        -e "TAURI_SIGNING_PRIVATE_KEY_PASSWORD=$(cat "$ROOT/.secrets/spcycle.key.password")"
    )
fi

mkdir -p out

echo "==> building the image"
docker build -f Dockerfile.appimage -t spcycle-appimage .

echo "==> building the appimage"
docker run --rm "${SIGN[@]}" -v "$ROOT/out:/out" spcycle-appimage

echo
ls -la out/
