#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:-8.0.4}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/artifacts"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$DIST"

WIN_URL="https://fastdl.mongodb.org/windows/mongodb-windows-x86_64-${VERSION}.zip"
LINUX_URL="https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2204-${VERSION}.tgz"

need() { command -v "$1" >/dev/null || { echo "need $1" >&2; exit 1; }; }
need curl; need python3; need tar

mkzip() { # mkzip <out.zip> <file-on-disk> <name-in-zip>
  python3 - "$@" <<'PY'
import os, sys, zipfile
out, src, name = sys.argv[1], sys.argv[2], sys.argv[3]
info = zipfile.ZipInfo(name)
info.external_attr = (0o755 << 16) | 0o600
info.compress_type = zipfile.ZIP_DEFLATED
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    z.writestr(info, open(src, "rb").read())
print(f"    wrote {out} ({os.path.getsize(out)/1048576:.1f} MiB)")
PY
}

# ---- windows: pull one member out of a remote zip ----
echo "==> windows: extracting bin/mongod.exe from $WIN_URL"
python3 - "$WIN_URL" "$WORK/mongod.exe" <<'PY'
import struct, subprocess, sys, zlib

url, out = sys.argv[1], sys.argv[2]

def fetch(start, end):
    r = subprocess.run(["curl", "-sL", "-r", f"{start}-{end}", url],
                       capture_output=True, check=True)
    return r.stdout

total = int(subprocess.run(
    ["curl", "-sIL", url], capture_output=True, check=True, text=True
).stdout.lower().rsplit("content-length:", 1)[1].split()[0])

# The central directory sits at the tail; 256 KiB covers it for this archive.
tail = fetch(max(0, total - 262144), total - 1)
if tail.rfind(b"PK\x05\x06") == -1:
    sys.exit("no end-of-central-directory found; archive layout changed")

target, offset, csize, usize, method = None, None, None, None, None
p = 0
while True:
    p = tail.find(b"PK\x01\x02", p)
    if p == -1:
        break
    m, = struct.unpack("<H", tail[p + 10 : p + 12])
    cs, us = struct.unpack("<II", tail[p + 20 : p + 28])
    nlen, elen, clen = struct.unpack("<HHH", tail[p + 28 : p + 34])
    off, = struct.unpack("<I", tail[p + 42 : p + 46])
    name = tail[p + 46 : p + 46 + nlen].decode("utf8", "replace")
    if name.endswith("bin/mongod.exe"):
        target, offset, csize, usize, method = name, off, cs, us, m
        break
    p += 1

if target is None:
    sys.exit("bin/mongod.exe not present in the archive")
print(f"    {target}: {csize/1048576:.1f} MiB packed, {usize/1048576:.1f} MiB raw")

head = fetch(offset, offset + 29)
nlen, elen = struct.unpack("<HH", head[26:30])
start = offset + 30 + nlen + elen
blob = fetch(start, start + csize - 1)
if len(blob) != csize:
    sys.exit(f"short read: got {len(blob)} of {csize} bytes")

data = zlib.decompress(blob, -15) if method == 8 else blob
if len(data) != usize:
    sys.exit(f"inflated to {len(data)}, expected {usize}")
open(out, "wb").write(data)
PY

mkzip "$DIST/mongod-windows.zip" "$WORK/mongod.exe" "bin/mongod.exe"

# ---- linux: whole tarball, it is small and not seekable ----
echo "==> linux: fetching $LINUX_URL"
curl -fL --progress-bar -o "$WORK/mongo.tgz" "$LINUX_URL"
tar -xzf "$WORK/mongo.tgz" -C "$WORK" --wildcards --no-anchored 'bin/mongod' --strip-components=1
[ -f "$WORK/bin/mongod" ] || { echo "bin/mongod not found in the tarball" >&2; exit 1; }
chmod +x "$WORK/bin/mongod"
mkzip "$DIST/mongod-linux.zip" "$WORK/bin/mongod" "bin/mongod"

# ---- manifest entries ----
echo
echo "==> paste into tools/components-<platform>.json (\"mongod\" key):"
for plat in windows linux; do
  f="$DIST/mongod-$plat.zip"
  printf '  %s: {"url": "https://github.com/itsOwen/spcycle-launcher/releases/latest/download/mongod-%s.zip", "sha256": "%s", "size": %s}\n' \
    "$plat" "$plat" "$(sha256sum "$f" | cut -d' ' -f1)" "$(stat -c%s "$f")"
done
ls -la "$DIST"
