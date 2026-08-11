#!/usr/bin/env bash
# build a release on this machine and optionally publish it: bundle, installer, .sig,
# mongod archives, latest.json.
#
# both manifests and both mongod archives must go on every release or component
# installs break, so run this on each platform into the same out/ then publish once.
#
#     ./tools/release.sh v0.1.0 [--publish]
set -euo pipefail

TAG=${1:?usage: release.sh <tag> [--publish]}
PUBLISH=${2:-}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/out"
KEY="$ROOT/.secrets/spcycle.key"

cd "$ROOT"
mkdir -p "$OUT"

[ -f "$KEY" ] || { echo "no signing key at $KEY. an unsigned build is one every install rejects." >&2; exit 1; }

# bundled into the app; tauri build fails late and obscurely without it
[ -f src-tauri/resources/depot.blob ] || {
    echo "src-tauri/resources/depot.blob is missing. run ./tools/fetch-depot-blob.sh first." >&2
    exit 1
}

case "$(uname -s)" in
  Linux)          PLATFORM=linux;   OURS='*.AppImage' ;;
  MINGW*|MSYS*|CYGWIN*) PLATFORM=windows; OURS='*-setup.exe' ;;
  *) echo "unsupported platform $(uname -s)" >&2; exit 1 ;;
esac

echo "==> gates"
pnpm typecheck
cargo test --manifest-path src-tauri/Cargo.toml

# out/ is shared across platforms so it is never wiped, but this platform's older
# artifact must go or make-latest-json.sh publishes it under the new version
find "$OUT" -maxdepth 1 \( -name "$OURS" -o -name "$OURS.sig" \) -type f -delete

# both are built in docker, on ubuntu, whatever this machine is: the appimage
# because glibc symbols are versioned and one linked against a rolling distro
# starts nowhere older, the installer because cargo-xwin plus NSIS cross-builds it
# without windows. one machine cuts the whole release.
if [ "$PLATFORM" = linux ]; then
    echo "==> building the appimage (ubuntu 22.04, via docker)"
    ./tools/build-appimage.sh --sign
    echo "==> building the windows installer (cargo-xwin, via docker)"
    ./tools/build-windows.sh --sign
else
    echo "==> building the installer"
    TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" \
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$ROOT/.secrets/spcycle.key.password")" \
      pnpm tauri build --bundles nsis

    echo "==> collecting"
    while IFS= read -r f; do
        cp "$f" "$OUT/"
        [ -f "$f.sig" ] && cp "$f.sig" "$OUT/"
    done < <(find src-tauri/target/release/bundle -name '*-setup.exe' -type f)
fi

# the signature is what the updater checks; an installer without one is one every
# client refuses, and make-latest-json.sh would rather fail than emit half a file
for want in '*.AppImage' '*-setup.exe'; do
    built=$(find "$OUT" -maxdepth 1 -name "$want" -type f -print -quit)
    [ -n "$built" ] || continue
    [ -f "$built.sig" ] || { echo "$built has no .sig beside it" >&2; exit 1; }
    echo "    $(basename "$built")"
done
find "$OUT" -maxdepth 1 \( -name '*.AppImage' -o -name '*-setup.exe' \) -type f -print -quit \
    | grep -q . || { echo "no installer was produced" >&2; exit 1; }

# the components are not here on purpose. they live on their own fixed tag, so a
# release that forgot one cannot break installs for everyone on that platform.
# see tools/publish-components.sh.

echo "==> updater manifest"
./tools/make-latest-json.sh "$TAG" "$OUT"

# removed first: the glob would otherwise hash the previous run's SHA256SUMS
# into the new one before the redirection truncates it
( cd "$OUT" && rm -f SHA256SUMS && sha256sum -- * > SHA256SUMS )
ls -la "$OUT"

if [ "$PUBLISH" = "--publish" ]; then
  echo "==> publishing $TAG"
  # --clobber belongs to `upload`, not `create`. creating is separate so a second
  # platform can publish into a release the first one already made.
  gh release view "$TAG" >/dev/null 2>&1 || gh release create "$TAG" --generate-notes
  gh release upload "$TAG" "$OUT"/* --clobber
else
  echo
  echo "built into out/. publish with:"
  echo "    ./tools/release.sh $TAG --publish"
fi
