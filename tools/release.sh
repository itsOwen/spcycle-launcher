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

if [ "$PLATFORM" = linux ]; then
    # ubuntu 22.04 in docker, never the host. glibc symbols are versioned, so an
    # appimage linked here starts on this machine and nowhere older — which is
    # most of the people downloading it. lands straight in out/, signed.
    echo "==> building the appimage (ubuntu 22.04, via docker)"
    ./tools/build-appimage.sh --sign
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
built=$(find "$OUT" -maxdepth 1 -name "$OURS" -type f -print -quit)
[ -n "$built" ] || { echo "no installer was produced for $PLATFORM" >&2; exit 1; }
[ -f "$built.sig" ] || { echo "$built has no .sig beside it" >&2; exit 1; }
echo "    $(basename "$built")"

echo "==> component assets"
./tools/repack-mongod.sh
for plat in windows linux; do
  want=$(python3 -c "import json,sys; print(json.load(open(f'tools/components-{sys.argv[1]}.json'))['files']['mongod']['sha256'])" "$plat")
  got=$(sha256sum "artifacts/mongod-$plat.zip" | cut -d' ' -f1)
  if [ "$want" != "$got" ]; then
    echo "mongod-$plat.zip hashes to $got but components-$plat.json pins $want." >&2
    echo "re-run tools/repack-mongod.sh and commit the updated manifest." >&2
    exit 1
  fi
  cp "artifacts/mongod-$plat.zip" "$OUT/"
done
cp tools/components-windows.json tools/components-linux.json "$OUT/"

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
