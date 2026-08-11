#!/usr/bin/env bash
# build a release on this machine and, optionally, publish it.
#
# builds the bundle for whatever platform this is, collects the installer and the
# .sig the updater checks against, rebuilds the mongod archives and checks them
# against the manifests, then writes latest.json.
#
# the launcher reads components-<platform>.json from releases/latest/download, so
# both manifests and both mongod archives go on every release or component
# installs break for everyone. run this on each platform into the same out/ dir,
# then publish once.
#
#     ./tools/release.sh v0.1.0            # build into out/
#     ./tools/release.sh v0.1.0 --publish  # and create the github release
set -euo pipefail

TAG=${1:?usage: release.sh <tag> [--publish]}
PUBLISH=${2:-}
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/out"
KEY="$ROOT/.secrets/spcycle.key"

cd "$ROOT"
mkdir -p "$OUT"

[ -f "$KEY" ] || { echo "no signing key at $KEY. an unsigned build is one every install rejects." >&2; exit 1; }

case "$(uname -s)" in
  Linux)          BUNDLE=appimage ;;
  MINGW*|MSYS*|CYGWIN*) BUNDLE=nsis ;;
  *) echo "unsupported platform $(uname -s)" >&2; exit 1 ;;
esac

echo "==> gates"
pnpm typecheck
cargo test --manifest-path src-tauri/Cargo.toml

echo "==> building $BUNDLE"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$ROOT/.secrets/spcycle.key.password")" \
  pnpm tauri build --bundles "$BUNDLE"

echo "==> collecting"
found=0
for pattern in '*-setup.exe' '*.AppImage'; do
  while IFS= read -r f; do
    cp "$f" "$OUT/"
    [ -f "$f.sig" ] && cp "$f.sig" "$OUT/"
    found=1
  done < <(find src-tauri/target/release/bundle -name "$pattern" -type f)
done
[ "$found" = 1 ] || { echo "no installer was produced" >&2; exit 1; }

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

( cd "$OUT" && sha256sum -- * > SHA256SUMS )
ls -la "$OUT"

if [ "$PUBLISH" = "--publish" ]; then
  echo "==> publishing $TAG"
  gh release create "$TAG" "$OUT"/* --generate-notes --clobber
else
  echo
  echo "built into out/. publish with:"
  echo "    ./tools/release.sh $TAG --publish"
fi
