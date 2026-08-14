#!/usr/bin/env bash

set -euo pipefail

cd "$(dirname "$0")/.."
OUT=src-tauri/resources/depot.blob
INFO="https://api.playrecycle.com/Client/DLInfo?version=frontier_mp"

if [[ -f $OUT && ${1:-} != --force ]]; then
    echo "$OUT already exists. Pass --force to replace it."
    exit 0
fi

if [[ -n ${DEPOT_BLOB_URL:-} ]]; then
    uri=$DEPOT_BLOB_URL
    echo "==> using DEPOT_BLOB_URL"
else
    echo "==> asking for the blob location"
    uri=$(curl -fsS --max-time 20 -A 'spcycle-launcher/setup' "$INFO" |
        sed -n 's/.*"download_uri"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
fi

if [[ -z ${uri:-} ]]; then
    echo "could not read download_uri from $INFO" >&2
    exit 1
fi
case $uri in
    https://*) ;;
    *) echo "refusing a non-https blob URI: $uri" >&2; exit 1 ;;
esac
echo "    $uri"

echo "==> downloading"
mkdir -p "$(dirname "$OUT")"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
curl -fsS --max-time 300 -A 'spcycle-launcher/setup' "$uri" -o "$tmp"

# fail here rather than at runtime: a wrong blob is cheaper to catch now
echo "==> checking it is a zlib stream wrapping a Steam manifest"
python3 - "$tmp" <<'PY'
import struct, sys, zlib
raw = open(sys.argv[1], "rb").read()
if raw[:1] != b"\x78":
    sys.exit(f"not a zlib stream (first byte {raw[:1].hex()})")
blob = zlib.decompress(raw)
if len(blob) <= 32:
    sys.exit("too short to contain a depot key")
magic = struct.unpack_from("<I", blob, 32)[0]
if magic != 0x71F617D0:  # PROTOBUF_PAYLOAD_MAGIC
    sys.exit(f"manifest magic is {magic:#x}, expected 0x71f617d0")
print(f"    ok: {len(raw)} bytes compressed, {len(blob)} inflated")
PY

mv "$tmp" "$OUT"
trap - EXIT
sha256sum "$OUT"

cat <<'EOF'

==> next
Commit the blob (raw, not git-lfs: it is ~1.4 MB and will never be rewritten),
then confirm it is the build the launcher expects:

    cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture the_bundled_blob
EOF
