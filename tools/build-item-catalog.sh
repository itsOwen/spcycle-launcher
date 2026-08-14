#!/usr/bin/env bash

set -euo pipefail

SRC="${1:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_JSON="$ROOT/src/assets/items.json"
OUT_ICONS="$ROOT/src/assets/items"

need() { command -v "$1" >/dev/null || { echo "need $1" >&2; exit 1; }; }
need magick; need python3

[ -n "$SRC" ] || { echo "usage: $0 /path/to/SingePlayer-Item-editor" >&2; exit 1; }
[ -d "$SRC/icons" ] || { echo "$SRC/icons is not there; wrong checkout?" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$OUT_ICONS"

# ---- merge, normalise, and decide every icon ----
python3 - "$SRC" "$OUT_JSON" "$WORK/icons.tsv" "$WORK/keep.txt" "$OUT_ICONS" <<'PY'
import json, os, re, sys

src, out_json, out_map, out_keep, icons_dir = sys.argv[1:6]

def load(name):
    with open(os.path.join(src, name), encoding="utf8") as f:
        d = json.load(f)
    return d.get("itemConfigs", d) if isinstance(d, dict) else d

# the underscored file is older but holds 22 entries the current one dropped
configs = load("_itemConfigs.json")
configs.update(load("itemConfigs.json"))
ids = {i["baseItemId"] for i in load("itemIds.json")}

# two real ids concatenated by a bad upstream edit; both exist separately in the union
ids.discard("Consumable_SmokeGrenade_01Consumable_GasGrenade_01")
configs.pop("Consumable_SmokeGrenade_01Consumable_GasGrenade_01", None)

RARITIES = {"common", "uncommon", "rare", "epic", "legendary", "exotic"}

def norm(s):
    return re.sub(r"[^a-z0-9]", "", (s or "").lower())

def pretty(item_id):
    # no config, so the id is all we have: Consumable_Health_01 -> Consumable Health 01
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", item_id.replace("_", " ")).strip()

# indexed by normalised stem. "ZeusBeam (2).png" stays distinct: it is different art
icon_files = {}
for f in sorted(os.listdir(os.path.join(src, "icons"))):
    stem, ext = os.path.splitext(f)
    if ext.lower() in (".png", ".jpg", ".jpeg", ".webp"):
        icon_files.setdefault(norm(stem), f)

catalog, mapping, iconless = {}, [], []

for item_id in sorted(ids | set(configs)):
    c = configs.get(item_id, {})
    name = (c.get("displayName") or "").strip() or pretty(item_id)
    category = (c.get("category") or "").strip() or "other"
    rarity = (c.get("rarity") or "").strip().lower()
    if rarity not in RARITIES:
        rarity = "common"

    weapon = item_id.startswith("WP_") or category == "weapons"
    stack = 1 if weapon else (int(c.get("maxStackSize") or 0) or 50)
    # -1 means no durability bar, and is what almost every config carries, weapons too
    durability = int(c.get("maxDurability", -1))

    catalog[item_id] = {
        "name": name,
        "category": category,
        "rarity": rarity,
        "stack": stack,
        "durability": durability,
    }

    # `Unknown.png` is the no-art sentinel, and one value has a newline inside it
    ref = re.sub(r"\s+", "", c.get("icon") or "")
    if ref.lower() in ("unknown.png", "unknown", ""):
        ref = ""
    # most files are named after the display name, which recovers far more art
    for key in (norm(os.path.splitext(ref)[0]) if ref else "", norm(name), norm(item_id)):
        if key and key in icon_files:
            slug = re.sub(r"[^A-Za-z0-9_.-]", "_", item_id)
            mapping.append(f"{icon_files[key]}\t{slug}")
            break
    else:
        iconless.append(item_id)

with open(out_json, "w", encoding="utf8") as f:
    json.dump(catalog, f, indent=1, sort_keys=True, ensure_ascii=False)
    f.write("\n")

with open(out_map, "w", encoding="utf8") as f:
    f.write("\n".join(mapping) + "\n")

with open(out_keep, "w", encoding="utf8") as f:
    f.write("\n".join(sorted(re.sub(r"[^A-Za-z0-9_.-]", "_", i) for i in catalog)) + "\n")

by_category = {}
for v in catalog.values():
    by_category[v["category"]] = by_category.get(v["category"], 0) + 1

# an item the editor has no art for may still have some from fetch-missing-icons.sh
already = {os.path.splitext(f)[0] for f in os.listdir(icons_dir)}
iconless = [i for i in iconless if re.sub(r"[^A-Za-z0-9_.-]", "_", i) not in already]

print(f"    {len(catalog)} items, {len(catalog) - len(iconless)} with art, {len(iconless)} without")
print("    " + "  ".join(f"{k} {v}" for k, v in sorted(by_category.items())))
if iconless:
    print("    no art: " + ", ".join(iconless[:8]) + (" …" if len(iconless) > 8 else ""))
PY

echo "==> converting icons"
while IFS=$'\t' read -r file slug; do
  [ -n "$file" ] || continue
  magick "$SRC/icons/$file" -resize 96x96\> -strip -quality 80 "$OUT_ICONS/$slug.webp"
done < "$WORK/icons.tsv"

# an item dropped upstream must not leave its art behind
gone=0
for f in "$OUT_ICONS"/*.webp; do
  [ -e "$f" ] || continue
  name="$(basename "$f" .webp)"
  grep -qxF "$name" "$WORK/keep.txt" || { rm -f "$f"; gone=$((gone + 1)); }
done
[ "$gone" -gt 0 ] && echo "    pruned $gone icon(s) for items no longer in the catalogue"

echo
echo "==> wrote $(basename "$OUT_JSON") ($(stat -c%s "$OUT_JSON") bytes)"
echo "    $(find "$OUT_ICONS" -name '*.webp' | wc -l) icons, $(du -sh "$OUT_ICONS" | cut -f1) total"
