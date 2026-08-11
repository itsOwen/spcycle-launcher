#!/usr/bin/env bash
# publish the component assets to their own fixed tag.
#
# they are deliberately not on an app release. the launcher reads them from a tag
# that never moves, so a launcher release cannot leave one behind and break
# component installs for everyone on that platform. it also means the components
# can be corrected without cutting a launcher release.
#
# the tag here must match COMPONENTS_TAG in src-tauri/src/components.rs.
#
#     ./tools/publish-components.sh [--publish]
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

    # the manifest must point at this tag, or the launcher fetches it from here and
    # is then sent somewhere else for the archive it names
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

# --latest=false matters: this tag carries no latest.json, and if github called it
# the newest release every updater would fetch a 404 instead of an update
gh release view "$TAG" >/dev/null 2>&1 || gh release create "$TAG" \
    --title "Components (${TAG#components-})" \
    --notes "The launcher fetches its components from this tag, not from the newest release. Not an app release — see the versioned tags for installers." \
    --latest=false
gh release upload "$TAG" "$STAGE"/* --clobber
echo "published to $TAG"
