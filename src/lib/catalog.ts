// the vendored item catalog, built by tools/build-item-catalog.sh
import items from "@/assets/items.json";
import { UNKNOWN, type ItemConfig } from "./stash";

export const CATALOG = items as Record<string, ItemConfig>;

// vite serves these from 'self', which is all the csp allows
const ICONS = import.meta.glob<string>("../assets/items/*.webp", {
  eager: true,
  query: "?url",
  import: "default",
});

// the same slug the build script writes, so ids with a space still resolve
const slug = (id: string) => id.replace(/[^A-Za-z0-9_.-]/g, "_");

const BY_SLUG: Record<string, string> = {};
for (const [path, url] of Object.entries(ICONS)) {
  BY_SLUG[path.slice(path.lastIndexOf("/") + 1, -5)] = url;
}

export const iconFor = (id: string): string | undefined => BY_SLUG[slug(id)];

export const configFor = (id: string): ItemConfig =>
  CATALOG[id] ?? { ...UNKNOWN, name: id };

export const CATEGORIES = [
  ...new Set(Object.values(CATALOG).map((c) => c.category)),
].sort();
