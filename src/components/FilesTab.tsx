import { bytes } from "@/lib/format";
import * as ipc from "@/lib/ipc";
import type { Snapshot } from "@/lib/ipc";
import { Btn, Panel, Row } from "./kit";

const BUSY: ipc.Phase[] = [
  "INSTALLING_COMPONENTS",
  "DOWNLOADING",
  "VERIFYING",
  "STARTING",
  "PLAYING",
  "UNINSTALLING",
  "UPDATING",
];

export function FilesTab({
  snap,
  onUninstall,
  onChanged,
}: {
  snap: Snapshot;
  onUninstall: () => void;
  onChanged: () => void;
}) {
  const busy = BUSY.includes(snap.phase);

  async function pick() {
    const chosen = await ipc.pickGameDirectory();
    if (chosen) onChanged();
  }

  return (
    <div className="space-y-4">
      <Panel
        title="Install"
        actions={
          <>
            <Btn onClick={pick} disabled={busy}>
              Change…
            </Btn>
            <Btn onClick={() => ipc.openGameFolder()}>Open</Btn>
          </>
        }
      >
        <Row label="Directory" value={snap.gameDirectory} mono={false} />
        <Row label="Game files" value={snap.install.gameFiles ? "complete" : "missing"} />
        <Row label="Manifest" value={snap.install.manifestId} />
        <Row label="Size on disk" value={bytes(snap.gameBytes)} />
        <Row label="Free space" value={bytes(snap.freeBytes)} />
      </Panel>

      <Panel title="Maintenance">
        <p className="mb-4 max-w-[56ch] leading-relaxed text-ink-2">
          Verify hashes every file against the depot manifest and re-downloads only what
          does not match. It is safe to run at any time and is the first thing to try if
          the game misbehaves.
        </p>
        <div className="flex gap-2">
          <Btn onClick={() => ipc.verifyAndRepair()} disabled={busy}>
            Verify &amp; repair
          </Btn>
          <Btn onClick={onUninstall} disabled={busy} tone="danger">
            Uninstall…
          </Btn>
        </div>
      </Panel>

      <Panel
        title="Components"
        actions={<Btn onClick={() => ipc.openLauncherFolder()}>Open data folder</Btn>}
      >
        <Row label="Version" value={snap.componentsVersion ?? "not installed"} />
        <Row label="State" value={snap.install.components ? "complete" : "missing"} />
        <p className="mt-3 mb-3 max-w-[56ch] leading-relaxed text-ink-2">
          The local server, the client loader and MongoDB. Reinstalling re-downloads
          them and checks each against its checksum.
        </p>
        <Btn
          onClick={() => ipc.installComponents().catch(() => {})}
          disabled={busy}
        >
          Reinstall components
        </Btn>
      </Panel>
    </div>
  );
}
