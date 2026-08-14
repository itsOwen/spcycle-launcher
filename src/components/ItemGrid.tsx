import { iconFor } from "@/lib/catalog";
import type { ItemConfig, Rarity } from "@/lib/stash";

// the only non-amber hues in the app, and only ever the 2px rule on a tile
const TIER: Record<Rarity, string> = {
  common: "var(--color-tier-common)",
  uncommon: "var(--color-tier-uncommon)",
  rare: "var(--color-tier-rare)",
  epic: "var(--color-tier-epic)",
  legendary: "var(--color-tier-legendary)",
  exotic: "var(--color-tier-exotic)",
};

export interface Entry {
  key: string;
  id: string;
  cfg: ItemConfig;
  amount?: number;
}

export function ItemGrid({
  entries,
  onPick,
}: {
  entries: Entry[];
  onPick: (e: Entry) => void;
}) {
  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(196px,1fr))] gap-2">
      {entries.map((e, i) => {
        const icon = iconFor(e.id);
        return (
          <li key={e.key}>
            <button
              onClick={() => onPick(e)}
              title={e.id}
              style={{
                borderLeftColor: TIER[e.cfg.rarity],
                // capped: past the first screenful the stagger is just latency
                animationDelay: `${Math.min(i, 24) * 25}ms`,
              }}
              className="sweep-in panel-hover flex w-full items-center gap-2.5 border-l-2 bg-panel-2 p-2 text-left transition-colors hover:bg-panel-3"
            >
              {icon ? (
                <img
                  src={icon}
                  alt=""
                  className="size-10 shrink-0 object-contain"
                />
              ) : (
                <span
                  aria-hidden
                  className="size-10 shrink-0 border border-hair bg-panel"
                />
              )}
              <span className="min-w-0 flex-1">
                <span className="block truncate text-ink">
                  {e.cfg.name || e.id}
                </span>
                <span className="hud block truncate text-ink-3">
                  {e.cfg.rarity}
                </span>
              </span>
              {e.amount !== undefined && (
                <span className="shrink-0 tabular-nums text-ink-2">
                  {e.amount}
                </span>
              )}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
