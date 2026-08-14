import { bytes, elapsed } from "@/lib/format";
import { useNow } from "@/hooks/useNow";
import type { Phase, Snapshot } from "@/lib/ipc";

// keyed by phase, so the headline and the button cannot disagree
const COPY: Record<Phase, { head: string; body: string }> = {
  NEEDS_COMPONENTS: {
    head: "Not installed",
    body: "Fetches the local server, the client loader and MongoDB. No administrator rights are needed.",
  },
  NEEDS_GAME: {
    head: "Game files missing",
    body: "Downloads the game from Steam's content servers. You can pause at any time and resume later.",
  },
  INSTALLING_COMPONENTS: {
    head: "Installing components",
    body: "Verifying each file against its checksum before it is moved into place.",
  },
  DOWNLOADING: {
    head: "Downloading game files",
    body: "Files are verified as they land. Pausing keeps everything already on disk.",
  },
  PAUSED: {
    head: "Paused",
    body: "Resuming re-checks what is on disk, then fetches only what is missing.",
  },
  VERIFYING: {
    head: "Verifying game files",
    body: "Hashing every file against the depot manifest and repairing any that do not match.",
  },
  READY: {
    head: "Ready to play",
    body: "Starts MongoDB and the local server, then launches the game against them.",
  },
  STARTING: {
    head: "Starting up",
    body: "Bringing up the local services in order. Watch the lamps on the left.",
  },
  PLAYING: {
    head: "Playing",
    body: "Stopping closes the game and shuts the local server and database down cleanly.",
  },
  UNINSTALLING: {
    head: "Removing",
    body: "Deleting only the files and directories this launcher can prove it created.",
  },
  UPDATING: { head: "Updating the launcher", body: "The launcher will restart when it is done." },
  EDITING: {
    head: "Editing the stash",
    body: "The launcher is holding the database while the stash tab reads or writes it.",
  },
};

export function PlayTab({ snap, since }: { snap: Snapshot; since: number | null }) {
  const copy = COPY[snap.phase];
  const playing = snap.phase === "PLAYING" && since !== null;
  const now = useNow(playing);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-baseline gap-3">
        <h1 className="text-[1.75rem] leading-none tracking-tight text-ink">
          Fortuna&nbsp;III
        </h1>
        <span className="hud text-ink-3">The Cycle: Frontier — Singleplayer</span>
      </div>
      <div className="mt-3 h-px w-full bg-hair" />

      <div className="mt-6">
        <p className="hud text-amber">{copy.head}</p>
        <p className="mt-2 max-w-[52ch] leading-relaxed text-ink-2">{copy.body}</p>
      </div>

      <dl className="mt-6 flex gap-8">
        <div>
          <dt className="hud text-ink-3">On disk</dt>
          <dd className="mt-1 tabular-nums text-ink">{bytes(snap.gameBytes)}</dd>
        </div>
        <div>
          <dt className="hud text-ink-3">Free</dt>
          <dd className="mt-1 tabular-nums text-ink">{bytes(snap.freeBytes)}</dd>
        </div>
        {playing && (
          <div>
            <dt className="hud text-ink-3">Elapsed</dt>
            <dd className="mt-1 tabular-nums text-amber">{elapsed(since!, now)}</dd>
          </div>
        )}
      </dl>
    </div>
  );
}
