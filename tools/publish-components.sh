#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TAG=components-b8
PUBLISH=${1:-}
STAGE="$ROOT/artifacts/components"

grep -q "\"$TAG\"" src-tauri/src/components.rs || {
    echo "src-tauri/src/components.rs does not mention $TAG. update one or the other." >&2
    exit 1
}

mkdir -p "$STAGE"
./tools/repack-mongod.sh

for plat in windows linux; do
    want=$(python3 -c "import json,sys; print(json.load(open(f'tools/components-{sys.argv[1]}.json'))['files']['mongod']['sha256'])" "$plat")
    got=$(sha256sum "artifacts/mongod-$plat.zip" | cut -d' ' -f1)
    [ "$want" = "$got" ] || {
        echo "mongod-$plat.zip hashes to $got but components-$plat.json pins $want." >&2
        echo "re-run tools/repack-mongod.sh and commit the updated manifest." >&2
        exit 1
    }
    cp "artifacts/mongod-$plat.zip" "$STAGE/"

    grep -q "releases/download/$TAG/mongod-$plat.zip" "tools/components-$plat.json" || {
        echo "components-$plat.json does not point its mongod url at $TAG." >&2
        exit 1
    }
done
cp tools/components-windows.json tools/components-linux.json "$STAGE/"

ls -la "$STAGE"

if [ "$PUBLISH" != "--publish" ]; then
    echo
    echo "staged in $STAGE. publish with:"
    echo "    ./tools/publish-components.sh --publish"
    exit 0
fi

gh release view "$TAG" >/dev/null 2>&1 || gh release create "$TAG" \
    --title "Components (${TAG#components-})" \
    --notes "The launcher fetches its components from this tag, not from the newest release. Not an app release — see the versioned tags for installers." \
    --latest=false
gh release upload "$TAG" "$STAGE"/* --clobber
echo "published to $TAG"
