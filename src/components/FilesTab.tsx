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
  onError,
}: {
  snap: Snapshot;
  onUninstall: () => void;
  onChanged: () => void;
  onError: (message: string) => void;
}) {
  const busy = BUSY.includes(snap.phase);

  // claim() rejects while anything else runs, so a swallowed rejection was a
  // button that visibly did nothing
  const run = (p: Promise<unknown>) => {
    p.catch((e: unknown) => onError(String(e)));
  };

  function pick() {
    ipc.pickGameDirectory().then(
      (chosen) => {
        if (chosen) onChanged();
      },
      (e: unknown) => onError(String(e)),
    );
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
            <Btn onClick={() => run(ipc.openGameFolder())}>Open</Btn>
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
          <Btn onClick={() => run(ipc.verifyAndRepair())} disabled={busy}>
            Verify &amp; repair
          </Btn>
          <Btn onClick={onUninstall} disabled={busy} tone="danger">
            Uninstall…
          </Btn>
        </div>
      </Panel>

      <Panel
        title="Components"
        actions={<Btn onClick={() => run(ipc.openLauncherFolder())}>Open data folder</Btn>}
      >
        <Row label="Version" value={snap.componentsVersion ?? "not installed"} />
        <Row label="State" value={snap.install.components ? "complete" : "missing"} />
        <p className="mt-3 mb-3 max-w-[56ch] leading-relaxed text-ink-2">
          The local server, the client loader and MongoDB. Reinstalling re-downloads
          them and checks each against its checksum.
        </p>
        <Btn
          onClick={() => run(ipc.installComponents())}
          disabled={busy}
        >
          Reinstall components
        </Btn>
      </Panel>
    </div>
  );
}
