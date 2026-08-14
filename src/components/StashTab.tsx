import { useEffect, useState } from "react";
import { bytes } from "@/lib/format";
import { CATEGORIES, configFor, CATALOG } from "@/lib/catalog";
import * as ipc from "@/lib/ipc";
import type { Backup, Snapshot } from "@/lib/ipc";
import {
  addItems,
  editItem,
  removeItem,
  type Balance,
  type Item,
} from "@/lib/stash";
import { Btn, Empty } from "./kit";
import { ItemDialog } from "./ItemDialog";
import { ItemGrid, type Entry } from "./ItemGrid";

const VIEWS = ["stash", "add", "backups"] as const;
type View = (typeof VIEWS)[number];

const NO_BALANCE: Balance = { AU: 0, SC: 0, IN: 0 };

const COINS: { key: keyof Balance; label: string }[] = [
  { key: "SC", label: "K-Marks" },
  { key: "AU", label: "Aurum" },
  { key: "IN", label: "Insurance" },
];

// reading is fine mid-session; the client would overwrite a save
const PLAYING: ipc.Phase[] = ["PLAYING", "STARTING"];

// App unmounts the tab on every rail switch, so unsaved edits have to outlive it
let draft: { profile: string; items: Item[]; balance: Balance } | null = null;

// strictmode mounts effects twice, and the second call would hit the busy claim
let inflight: Promise<ipc.StashData> | null = null;

function loadOnce(): Promise<ipc.StashData> {
  inflight ??= ipc.stashLoad().finally(() => {
    inflight = null;
  });
  return inflight;
}

export function StashTab({
  snap,
  onError,
  onNotify,
}: {
  snap: Snapshot;
  onError: (m: string) => void;
  onNotify: (m: string, level: 0 | 1 | 2) => void;
}) {
  const playing = PLAYING.includes(snap.phase);

  const [view, setView] = useState<View>("stash");
  const [status, setStatus] = useState("Reading the stash…");
  const [data, setData] = useState<ipc.StashData | null>(null);
  const [profile, setProfile] = useState("");
  const [items, setItems] = useState<Item[]>([]);
  const [balance, setBalance] = useState<Balance>(NO_BALANCE);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [backups, setBackups] = useState<Backup[]>([]);
  const [open, setOpen] = useState<Item | null>(null);

  const [query, setQuery] = useState("");
  const [category, setCategory] = useState("all");
  const [qty, setQty] = useState(1);
  const [renaming, setRenaming] = useState<{ name: string; to: string } | null>(
    null,
  );
  const [confirming, setConfirming] = useState<string | null>(null);
  const dbUp = snap.services.mongo === "up";

  useEffect(() => {
    let live = true;
    loadOnce().then(
      (fresh) => {
        if (!live) return;
        adopt(fresh);
        setStatus("");
      },
      (e: unknown) => live && setStatus(String(e)),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (dirty) draft = { profile, items, balance };
  }, [dirty, profile, items, balance]);

  useEffect(() => {
    if (view !== "backups") return;
    refreshBackups();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, saving]);

  function refreshBackups() {
    ipc.stashBackups().then(setBackups, (e: unknown) => onError(String(e)));
  }

  function fill(p: { items: string; balance: string | null } | undefined) {
    if (!p) return;
    try {
      setItems(JSON.parse(p.items) as Item[]);
      setBalance(p.balance ? (JSON.parse(p.balance) as Balance) : NO_BALANCE);
      setDirty(false);
    } catch (e) {
      setStatus(`The stash could not be read: ${String(e)}`);
    }
  }

  function adopt(d: ipc.StashData) {
    setData(d);
    if (draft) {
      setProfile(draft.profile);
      setItems(draft.items);
      setBalance(draft.balance);
      setDirty(true);
      return;
    }
    setProfile(d.profiles[0]?.playfabId ?? "");
    fill(d.profiles[0]);
  }

  function reload() {
    draft = null;
    setDirty(false);
    setStatus("Reading the stash…");
    // not loadOnce: reloading has to re-read, not join a request already in flight
    ipc.stashLoad().then(
      (fresh) => {
        adopt(fresh);
        setStatus("");
      },
      (e: unknown) => setStatus(String(e)),
    );
  }

  async function snapshot() {
    try {
      const name = await ipc.stashSnapshot(
        profile,
        JSON.stringify(items),
        JSON.stringify(balance),
      );
      refreshBackups();
      onNotify(`Saved a copy of ${items.length} item(s) as ${name}.`, 1);
    } catch (e) {
      onError(String(e));
    }
  }

  async function stopDb() {
    try {
      await ipc.stashStopDb();
    } catch (e) {
      onError(String(e));
    }
  }

  function discard() {
    const p = data?.profiles.find((x) => x.playfabId === profile);
    if (!p) {
      reload();
      return;
    }
    draft = null;
    fill(p);
  }

  // every profile came back in one read, so switching is local
  function pickProfile(id: string) {
    setProfile(id);
    fill(data?.profiles.find((p) => p.playfabId === id));
  }

  async function save() {
    setSaving(true);
    try {
      const nextItems = JSON.stringify(items);
      const nextBalance = JSON.stringify(balance);
      await ipc.stashSave(profile, nextItems, nextBalance);
      draft = null;
      setDirty(false);
      // discard reverts to this, so it has to be what we just wrote
      setData((d) =>
        d && {
          profiles: d.profiles.map((p) =>
            p.playfabId === profile
              ? { ...p, items: nextItems, balance: nextBalance }
              : p,
          ),
        },
      );
      onNotify(`Saved ${items.length} item(s) to the stash.`, 1);
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function restore(name: string) {
    try {
      const backup = await ipc.stashBackupRead(name);
      const p =
        backup.profiles.find((x) => x.playfabId === profile) ??
        backup.profiles[0];
      if (!p) {
        onError(`${name} holds no character data.`);
        return;
      }
      // parsed before any state moves: a half-applied load crosses two characters
      const restored = JSON.parse(p.items) as Item[];
      const bal = p.balance ? (JSON.parse(p.balance) as Balance) : NO_BALANCE;
      setItems(restored);
      setBalance(bal);
      setProfile(p.playfabId);
      setDirty(true);
      setView("stash");
      onNotify(
        `Loaded ${restored.length} item(s) from ${name}. Save to write it back.`,
        0,
      );
    } catch (e) {
      onError(String(e));
    }
  }

  async function commitRename() {
    if (!renaming) return;
    try {
      await ipc.stashBackupRename(renaming.name, renaming.to);
      setRenaming(null);
      refreshBackups();
    } catch (e) {
      onError(String(e));
    }
  }

  async function remove(name: string) {
    try {
      await ipc.stashBackupDelete(name);
      setConfirming(null);
      refreshBackups();
    } catch (e) {
      onError(String(e));
    }
  }

  const source: Entry[] =
    view === "add"
      ? Object.keys(CATALOG).map((id) => ({ key: id, id, cfg: configFor(id) }))
      : items.map((it) => ({
          key: it.itemId,
          id: it.baseItemId,
          cfg: configFor(it.baseItemId),
          amount: it.amount,
        }));

  const needle = query.trim().toLowerCase();
  const shown = source.filter(
    (e) =>
      (category === "all" || e.cfg.category === category) &&
      (needle === "" ||
        e.cfg.name.toLowerCase().includes(needle) ||
        e.id.toLowerCase().includes(needle)),
  );

  function pick(e: Entry) {
    if (saving) return;
    if (view === "add") {
      setItems(addItems(items, e.id, e.cfg, qty));
      setDirty(true);
      onNotify(`Added ${qty} × ${e.cfg.name || e.id}.`, 0);
      return;
    }
    setOpen(items.find((it) => it.itemId === e.key) ?? null);
  }

  return (
    <div className="flex h-full flex-col gap-3">
      {playing && (
        <p className="max-w-[72ch] leading-relaxed text-led-fail">
          The game is running, so the stash is read-only. It keeps its own copy in
          memory and would overwrite anything saved now.
        </p>
      )}

      <div className="flex items-center gap-2">
        <div className="flex border border-hair">
          {VIEWS.map((v) => (
            <button
              key={v}
              onClick={() => setView(v)}
              className={`hud px-2.5 py-1 transition-colors ${
                view === v
                  ? "bg-amber-wash text-amber"
                  : "text-ink-3 hover:bg-panel-2 hover:text-ink-2 disabled:text-hair-lit disabled:hover:bg-transparent"
              }`}
            >
              {v}
            </button>
          ))}
        </div>

        <Btn onClick={save} disabled={playing || !dirty || saving}>
          {saving ? "Saving" : dirty ? "Save •" : "Save"}
        </Btn>
        {dirty && !saving ? (
          <Btn onClick={discard}>Discard</Btn>
        ) : (
          <Btn onClick={reload} disabled={saving}>
            Reload
          </Btn>
        )}
        <Btn onClick={snapshot} disabled={items.length === 0}>
          Snapshot
        </Btn>

        {view !== "backups" && (
          <input
            className="field max-w-[240px]"
            placeholder="search…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        )}

        {view === "add" && (
          <label className="flex shrink-0 items-center gap-2">
            <span className="hud text-ink-3">Qty</span>
            <input
              className="field w-20"
              type="number"
              min={1}
              value={qty}
              onChange={(e) => setQty(Math.max(1, Number(e.target.value) || 1))}
            />
          </label>
        )}

        <div className="ml-auto flex shrink-0 items-center gap-2">
          <span
            aria-hidden
            className={`size-1.5 ${dbUp ? "bg-led-up" : "bg-led-idle"}`}
          />
          <span className="hud text-ink-3">
            {dbUp ? "database on" : "database off"}
          </span>
          {dbUp && !playing && <Btn onClick={stopDb}>Stop</Btn>}
        </div>

        {(data?.profiles.length ?? 0) > 1 && (
          <select
            className="field w-[150px]"
            value={profile}
            // switching would drop unsaved edits on the floor
            disabled={dirty}
            onChange={(e) => pickProfile(e.target.value)}
          >
            {data?.profiles.map((p) => (
              <option key={p.playfabId} value={p.playfabId}>
                {p.playfabId}
              </option>
            ))}
          </select>
        )}
      </div>

      {view !== "backups" && (
        <div className="flex items-center gap-4 border border-hair bg-panel px-4 py-2">
          {COINS.map(({ key, label }) => (
            <label key={key} className="flex items-center gap-2">
              <span className="hud text-ink-3">{label}</span>
              <input
                className="field w-28"
                type="number"
                min={0}
                value={balance[key]}
                disabled={saving}
                onChange={(e) => {
                  setBalance({
                    ...balance,
                    [key]: Math.max(0, Math.floor(Number(e.target.value)) || 0),
                  });
                  setDirty(true);
                }}
              />
            </label>
          ))}
          <span className="hud ml-auto text-ink-3">{items.length} items</span>
        </div>
      )}

      <section className="flex min-h-0 flex-1 flex-col border border-hair bg-panel">
        {view !== "backups" && (
          <header className="flex flex-wrap items-center gap-1 border-b border-hair px-3 py-2">
            {["all", ...CATEGORIES].map((c) => (
              <button
                key={c}
                onClick={() => setCategory(c)}
                className={`hud px-2.5 py-1 transition-colors ${
                  category === c
                    ? "bg-amber-wash text-amber"
                    : "text-ink-3 hover:bg-panel-2 hover:text-ink-2"
                }`}
              >
                {c}
              </button>
            ))}
            <span className="hud ml-auto text-ink-3">{shown.length}</span>
          </header>
        )}

        <div className="min-h-0 flex-1 overflow-auto p-3">
          {view === "backups" ? (
            backups.length === 0 ? (
              <Empty>No backups yet. One is written every time you save.</Empty>
            ) : (
              <>
                <p className="mb-3 max-w-[78ch] leading-relaxed text-ink-2">
                  One is written automatically before every save, holding what the
                  stash looked like beforehand — that is your undo. Snapshot writes
                  a copy of the stash as it is now. Only the ten newest are kept,
                  so rename one to keep it for good.
                </p>
                <ul className="space-y-2">
                  {backups.map((b) => (
                    <li
                      key={b.name}
                      className="flex items-center gap-3 border-l-2 border-l-amber bg-panel-2 p-3"
                    >
                      {renaming?.name === b.name ? (
                        <input
                          className="field max-w-[280px]"
                          autoFocus
                          value={renaming.to}
                          onChange={(e) =>
                            setRenaming({ name: b.name, to: e.target.value })
                          }
                          onKeyDown={(e) => {
                            if (e.key === "Enter") commitRename();
                            if (e.key === "Escape") setRenaming(null);
                          }}
                        />
                      ) : (
                        <span className="min-w-0 flex-1 truncate text-ink">
                          {b.name}
                        </span>
                      )}
                      <span className="hud shrink-0 tabular-nums text-ink-3">
                        {b.items} items
                      </span>
                      <span className="shrink-0 tabular-nums text-ink-3">
                        {bytes(b.bytes)}
                      </span>
                      {renaming?.name === b.name ? (
                        <>
                          <Btn onClick={commitRename}>Save name</Btn>
                          <Btn onClick={() => setRenaming(null)}>Cancel</Btn>
                        </>
                      ) : (
                        <>
                          <Btn onClick={() => restore(b.name)} disabled={playing}>
                            Load
                          </Btn>
                          <Btn
                            onClick={() =>
                              setRenaming({ name: b.name, to: b.name })
                            }
                          >
                            Rename
                          </Btn>
                          <Btn
                            tone="danger"
                            onClick={() =>
                              confirming === b.name
                                ? remove(b.name)
                                : setConfirming(b.name)
                            }
                          >
                            {confirming === b.name ? "Sure?" : "Delete"}
                          </Btn>
                        </>
                      )}
                    </li>
                  ))}
                </ul>
              </>
            )
          ) : status ? (
            <p className={status.endsWith("…") ? "text-ink-3" : "text-led-fail"}>
              {status}
            </p>
          ) : shown.length === 0 ? (
            <Empty>
              {source.length === 0
                ? "This stash is empty. Switch to ADD to put something in it."
                : "Nothing matches that."}
            </Empty>
          ) : (
            <ItemGrid entries={shown} onPick={pick} />
          )}
        </div>
      </section>

      {open && (
        <ItemDialog
          item={open}
          cfg={configFor(open.baseItemId)}
          onClose={() => setOpen(null)}
          onApply={(patch) => {
            setItems(editItem(items, open.itemId, patch));
            setDirty(true);
            setOpen(null);
          }}
          onRemove={() => {
            setItems(removeItem(items, open.itemId));
            setDirty(true);
            setOpen(null);
          }}
        />
      )}
    </div>
  );
}
