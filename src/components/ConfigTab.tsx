import { useEffect, useState } from "react";
import { bytes } from "@/lib/format";
import * as ipc from "@/lib/ipc";
import type { CompatInfo, DepotInfo } from "@/lib/ipc";
import { useSettings } from "@/hooks/useSettings";
import type { UpdateState } from "@/hooks/useUpdate";
import { Btn, Panel, Row } from "./kit";

// a labelled row wrapping a control, matching Row's alignment
function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: React.ReactNode;
  hint?: string;
}) {
  return (
    <div className="py-1.5">
      <div className="flex items-baseline gap-4">
        <span className="hud w-[120px] shrink-0 text-ink-3">{label}</span>
        <div className="min-w-0 flex-1">{children}</div>
      </div>
      {hint && <p className="mt-1 pl-[136px] text-ink-3">{hint}</p>}
    </div>
  );
}

function Toggle({
  on,
  onChange,
  children,
}: {
  on: boolean;
  onChange: (v: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <button
      onClick={() => onChange(!on)}
      role="switch"
      aria-checked={on}
      className="flex items-center gap-2.5 text-ink-2 transition-colors hover:text-ink"
    >
      <span
        className={`flex size-3.5 shrink-0 items-center justify-center border ${
          on ? "border-amber bg-amber" : "border-hair-lit"
        }`}
      >
        {on && (
          <svg width="8" height="8" viewBox="0 0 8 8" aria-hidden>
            <path
              d="M1 4l2 2 4-4"
              stroke="var(--color-amber-ink)"
              strokeWidth="1.5"
              fill="none"
            />
          </svg>
        )}
      </span>
      {children}
    </button>
  );
}

export function ConfigTab({
  version,
  update,
}: {
  version: string;
  update: UpdateState;
}) {
  const [depot, setDepot] = useState<DepotInfo | null>(null);
  const [depotErr, setDepotErr] = useState<string | null>(null);
  const [compat, setCompat] = useState<CompatInfo | null>(null);
  const { settings, update: set, loaded, error: settingsErr } = useSettings();

  useEffect(() => {
    ipc.depotInfo().then(setDepot, (e) => setDepotErr(String(e)));
    ipc.detectCompatTools().then(setCompat, () => setCompat(null));
  }, []);

  return (
    <div className="space-y-4">
      {settingsErr && <p className="text-led-fail">{settingsErr}</p>}

      {compat?.supported && (
        <Panel title="Compatibility">
          <Field
            label="Proton build"
            hint="The game signs in through Steam, and only Proton bridges to it."
          >
            <select
              className="field"
              value={settings.proton_path}
              disabled={!loaded || compat.proton.length === 0}
              onChange={(e) => set("proton_path", e.target.value)}
            >
              <option value="">
                {compat.proton.length > 0
                  ? `Automatic (${compat.proton[0].split("/").slice(-2, -1)[0]})`
                  : "None found"}
              </option>
              {compat.proton.map((p) => (
                <option key={p} value={p}>
                  {p.split("/").slice(-2, -1)[0]}
                </option>
              ))}
            </select>
          </Field>

          <Row label="Steam root" value={compat.steamRoot ?? "not found"} mono={false} />
          <Row label="Builds found" value={compat.proton.length} />
        </Panel>
      )}

      <Panel title="Launching">
        <Field label="Steam" hint="The game cannot sign in without it.">
          <Toggle on={settings.autorun_steam} onChange={(v) => set("autorun_steam", v)}>
            Start Steam automatically
          </Toggle>
        </Field>
        <Field label="Discord">
          <Toggle
            on={settings.discord_presence}
            onChange={(v) => set("discord_presence", v)}
          >
            Show what I am playing
          </Toggle>
        </Field>
        <Field
          label="MongoDB port"
          hint="0 picks a free port from 27055. Deliberately not 27017, so it never fights a MongoDB you installed yourself."
        >
          <input
            className="field"
            type="number"
            min={0}
            max={65535}
            value={settings.mongo_port}
            onChange={(e) => set("mongo_port", Number(e.target.value) || 0)}
          />
        </Field>
      </Panel>

      <Panel title="Depot">
        {depotErr && <p className="text-led-fail">{depotErr}</p>}
        {depot && (
          <>
            <Row label="Depot" value={depot.depotId} />
            <Row label="Manifest" value={depot.manifestId} />
            <Row label="Files" value={depot.files.toLocaleString()} />
            <Row label="Download" value={bytes(depot.compressedBytes)} />
            <Row label="On disk" value={bytes(depot.totalBytes)} />
            <Row
              label="Built"
              value={new Date(depot.createdAt * 1000).toISOString().slice(0, 10)}
            />
          </>
        )}
        {!depot && !depotErr && <p className="text-ink-3">Reading the bundled blob…</p>}
        <Field
          label="Override"
          hint="Advanced: a different depot blob to install from. Empty uses the one shipped with the launcher."
        >
          <input
            className="field"
            value={settings.depot_blob_path}
            onChange={(e) => set("depot_blob_path", e.target.value)}
            placeholder="(bundled)"
          />
        </Field>
      </Panel>

      <Panel title="Launcher">
        <Row label="Version" value={version || "—"} />
        <Row label="Update" value={update.label} mono={false} />
        <div className="mt-3 flex gap-2">
          <Btn
            onClick={update.available ? update.install : update.checkNow}
            disabled={update.busy}
          >
            {update.available ? "Install and restart" : "Check for updates"}
          </Btn>
          <Btn onClick={() => ipc.openLauncherFolder()}>Open data folder</Btn>
        </div>
      </Panel>
    </div>
  );
}
