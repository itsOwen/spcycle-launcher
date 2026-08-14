// item rules, ported from the community editor. no imports: the node test runs this file

export type Rarity =
  | "common"
  | "uncommon"
  | "rare"
  | "epic"
  | "legendary"
  | "exotic";

export interface ItemConfig {
  name: string;
  category: string;
  rarity: Rarity;
  stack: number;
  // -1 means no durability bar, which is what almost every item carries
  durability: number;
}

export interface Item {
  itemId: string;
  baseItemId: string;
  primaryVanityId: number;
  secondaryVanityId: number;
  amount: number;
  durability: number;
  modData: { m: unknown[] };
  rolledPerks: unknown[];
  insurance: string;
  insuranceOwnerPlayfabId: string;
  insuredAttachmentId: string;
  origin: { t: string; p: string; g: string };
}

export interface Balance {
  AU: number;
  SC: number;
  IN: number;
}

export const MAX_ADD = 9999;

export const UNKNOWN: ItemConfig = {
  name: "",
  category: "other",
  rarity: "common",
  stack: 50,
  durability: -1,
};

export function isWeapon(id: string, cfg: ItemConfig): boolean {
  return id.startsWith("WP_") || cfg.category === "weapons";
}

export function newItem(id: string, cfg: ItemConfig, amount: number): Item {
  return {
    itemId: crypto.randomUUID(),
    baseItemId: id,
    primaryVanityId: 0,
    secondaryVanityId: 0,
    amount,
    durability: cfg.durability,
    modData: { m: [] },
    rolledPerks: [],
    insurance: "",
    insuranceOwnerPlayfabId: "",
    insuredAttachmentId: "",
    origin: { t: "", p: "", g: "" },
  };
}

export function addItems(
  items: Item[],
  id: string,
  cfg: ItemConfig,
  qty: number,
): Item[] {
  // clamped here, not at the input: a weapon allocates one row per unit
  let left = Math.min(MAX_ADD, Math.max(1, Math.floor(qty) || 1));

  if (isWeapon(id, cfg)) {
    return [
      ...items,
      ...Array.from({ length: left }, () => newItem(id, cfg, 1)),
    ];
  }

  const cap = Math.max(1, Math.floor(cfg.stack) || 1);
  const out = items.map((it) => {
    if (left <= 0 || it.baseItemId !== id || it.amount >= cap) return it;
    const room = Math.min(cap - it.amount, left);
    left -= room;
    return { ...it, amount: it.amount + room };
  });

  while (left > 0) {
    const take = Math.min(left, cap);
    out.push(newItem(id, cfg, take));
    left -= take;
  }
  return out;
}

// only amount and durability are ours; everything the game wrote is spread through
export function editItem(
  items: Item[],
  itemId: string,
  patch: { amount?: number; durability?: number },
): Item[] {
  return items.map((it) => (it.itemId === itemId ? { ...it, ...patch } : it));
}

export function removeItem(items: Item[], itemId: string): Item[] {
  return items.filter((it) => it.itemId !== itemId);
}
