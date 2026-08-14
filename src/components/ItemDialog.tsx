import { useState } from "react";
import { iconFor } from "@/lib/catalog";
import { isWeapon, type Item, type ItemConfig } from "@/lib/stash";
import { useDialog } from "@/hooks/useDialog";
import { Btn } from "./kit";

export function ItemDialog({
  item,
  cfg,
  onApply,
  onRemove,
  onClose,
}: {
  item: Item;
  cfg: ItemConfig;
  onApply: (patch: { amount: number; durability: number }) => void;
  onRemove: () => void;
  onClose: () => void;
}) {
  const dialog = useDialog(onClose);
  const [amount, setAmount] = useState(item.amount);
  const [durability, setDurability] = useState(item.durability);

  // never below what the item holds: 68 entries fall back to a guessed stack of 50
  const cap = Math.max(
    isWeapon(item.baseItemId, cfg) ? 1 : cfg.stack,
    item.amount,
    1,
  );
  const icon = iconFor(item.baseItemId);

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-void/80 p-8">
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="item-title"
        className="flex w-[460px] flex-col border border-hair bg-panel"
      >
        <header className="flex items-center gap-3 border-b border-hair px-5 py-3">
          {icon && <img src={icon} alt="" className="size-8 object-contain" />}
          <div className="min-w-0">
            <h2 id="item-title" className="hud truncate text-ink">
              {cfg.name || item.baseItemId}
            </h2>
            <p className="hud truncate text-ink-3">
              {cfg.rarity} · {cfg.category}
            </p>
          </div>
        </header>

        <div className="space-y-3 p-5">
          <label className="flex items-baseline gap-4">
            <span className="hud w-[120px] shrink-0 text-ink-3">Amount</span>
            <input
              className="field"
              type="number"
              min={1}
              max={cap}
              value={amount}
              onChange={(e) => setAmount(Number(e.target.value))}
            />
          </label>

          <label className="flex items-baseline gap-4">
            <span className="hud w-[120px] shrink-0 text-ink-3">Durability</span>
            <input
              className="field"
              type="number"
              value={durability}
              onChange={(e) => setDurability(Number(e.target.value))}
            />
          </label>
          <p className="pl-[136px] text-ink-3">
            −1 leaves it as the game set it. Stacks hold up to {cap}.
          </p>

          <p className="hud select-text break-all text-ink-3">{item.itemId}</p>
        </div>

        <footer className="flex gap-2 border-t border-hair px-5 py-3">
          <Btn tone="danger" onClick={onRemove}>
            Remove
          </Btn>
          <div className="ml-auto flex gap-2">
            <Btn onClick={onClose}>Cancel</Btn>
            <Btn
              onClick={() =>
                onApply({
                  amount: Math.min(cap, Math.max(1, Math.floor(amount) || 1)),
                  durability: Number.isFinite(durability)
                    ? Math.floor(durability)
                    : -1,
                })
              }
            >
              Apply
            </Btn>
          </div>
        </footer>
      </div>
    </div>
  );
}
