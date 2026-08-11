import type { UpdateState } from "@/hooks/useUpdate";
import { Btn } from "./kit";

export function UpdateBanner({ update }: { update: UpdateState }) {
  const { available, busy, install, dismiss } = update;
  if (!available) return null;

  return (
    <div
      className="toast-in mb-4 flex shrink-0 items-center gap-3 border border-hair border-l-2 border-l-amber bg-panel-2 px-3 py-2"
      role="status"
      aria-live="polite"
    >
      <span className="min-w-0 flex-1 text-ink-2">
        Launcher {available.version} is available.
        {busy && " Downloading…"}
      </span>
      <Btn onClick={install} disabled={busy}>
        {busy ? "Installing…" : "Install and restart"}
      </Btn>
      <Btn onClick={dismiss} disabled={busy}>
        Later
      </Btn>
    </div>
  );
}
