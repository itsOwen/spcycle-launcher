import { useEffect, useState } from "react";
import { bytes } from "@/lib/format";
import * as ipc from "@/lib/ipc";
import type { UninstallItem } from "@/lib/ipc";
import { Btn } from "./kit";

export function UninstallDialog({
  onClose,
  onDone,
}: {
  onClose: () => void;
  onDone: () => void;
}) {
  const [items, setItems] = useState<UninstallItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    ipc.uninstallPlan().then(setItems, (e) => setError(String(e)));
  }, []);

  const removable = items?.filter((i) => i.removable) ?? [];
  const kept = items?.filter((i) => !i.removable) ?? [];
  const total = removable.reduce((sum, i) => sum + i.bytes, 0);

  async function run() {
    setRunning(true);
    try {
      await ipc.uninstallEverything();
      onDone();
    } catch (e) {
      setError(String(e));
      setRunning(false);
    }
  }

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-void/80 p-8">
      <div className="flex max-h-full w-[560px] flex-col border border-hair bg-panel">
        <header className="border-b border-hair px-5 py-3">
          <h2 className="hud text-ink">Uninstall</h2>
        </header>

        <div className="min-h-0 flex-1 overflow-auto p-5">
          {error && <p className="mb-4 text-led-fail">{error}</p>}
          {!items && !error && <p className="text-ink-3">Working out what is installed…</p>}

          {items && (
            <>
              <p className="mb-4 leading-relaxed text-ink-2">
                Only what this launcher can prove it created will be removed. Game files
                you had before, and anything else sharing these folders, are left alone.
              </p>

              <ul className="space-y-2">
                {removable.map((i) => (
                  <li key={i.label} className="border-l-2 border-l-amber bg-panel-2 p-3">
                    <div className="flex items-baseline gap-3">
                      <span className="text-ink">{i.label}</span>
                      <span className="ml-auto tabular-nums text-ink-2">
                        {i.bytes > 0 ? bytes(i.bytes) : ""}
                      </span>
                    </div>
                    <div className="hud mt-1 truncate text-ink-3" title={i.path}>
                      {i.path}
                    </div>
                    {i.note && <p className="mt-2 text-ink-2">{i.note}</p>}
                  </li>
                ))}
              </ul>

              {kept.length > 0 && (
                <>
                  <p className="hud mt-5 mb-2 text-ink-3">Left alone</p>
                  <ul className="space-y-2">
                    {kept.map((i) => (
                      <li key={i.label} className="border border-hair p-3 text-ink-3">
                        <div className="flex items-baseline gap-3">
                          <span>{i.label}</span>
                        </div>
                        {i.note && <p className="mt-1.5">{i.note}</p>}
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </>
          )}
        </div>

        <footer className="flex items-center gap-3 border-t border-hair px-5 py-3">
          {total > 0 && (
            <span className="hud tabular-nums text-ink-3">{bytes(total)} to free</span>
          )}
          <div className="ml-auto flex gap-2">
            <Btn onClick={onClose} disabled={running}>
              Cancel
            </Btn>
            <Btn onClick={run} disabled={running || removable.length === 0} tone="danger">
              {running ? "Removing…" : "Remove"}
            </Btn>
          </div>
        </footer>
      </div>
    </div>
  );
}
