#!/usr/bin/env bash
# build the updater manifest from the built artifacts. a wrong or missing signature
# fails silently for the user, so this errors rather than emit half a file.
#
#   ./tools/make-latest-json.sh v0.1.1 out
set -euo pipefail

TAG=${1:?usage: make-latest-json.sh <tag> <dir>}
DIR=${2:?usage: make-latest-json.sh <tag> <dir>}
VERSION=${TAG#v}
REPO=${GITHUB_REPOSITORY:-itsOwen/spcycle-launcher}
BASE="https://github.com/${REPO}/releases/download/${TAG}"

# platform key -> installer glob
declare -A GLOB=(
    [windows-x86_64]='*-setup.exe'
    [linux-x86_64]='*.AppImage'
)

entries=""
for platform in "${!GLOB[@]}"; do
    file=$(find "$DIR" -maxdepth 1 -name "${GLOB[$platform]}" -type f -print -quit)
    if [[ -z $file ]]; then
        echo "note: no artifact for $platform, skipping" >&2
        continue
    fi
    sig="$file.sig"
    if [[ ! -f $sig ]]; then
        echo "error: $file has no .sig beside it." >&2
        echo "       The build did not sign it, so the updater could never accept it." >&2
        echo "       Check TAURI_SIGNING_PRIVATE_KEY and createUpdaterArtifacts." >&2
        exit 1
    fi
    name=$(basename "$file")
    # github renames an asset's spaces to dots on upload, so the url it serves is
    # not the name on disk. get this wrong and every update 404s.
    url_name=${name// /.}
    signature=$(tr -d '\n' < "$sig")

    [[ -n $entries ]] && entries+=","
    entries+=$(printf '\n    "%s": { "signature": "%s", "url": "%s/%s" }' \
        "$platform" "$signature" "$BASE" "$url_name")
done

if [[ -z $entries ]]; then
    echo "error: no signed artifacts found in $DIR" >&2
    exit 1
fi

# rfc 3339, which is what the plugin expects for pub_date
printf '{\n  "version": "%s",\n  "notes": "See the release notes for %s.",\n  "pub_date": "%s",\n  "platforms": {%s\n  }\n}\n' \
    "$VERSION" "$TAG" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$entries" \
    > "$DIR/latest.json"

echo "wrote $DIR/latest.json"
cat "$DIR/latest.json"
