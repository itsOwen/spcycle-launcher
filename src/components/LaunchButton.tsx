import type { Phase } from "@/lib/ipc";

type Tone = "primary" | "danger" | "idle";

interface Spec {
  label: string;
  tone: Tone;
  // null = the phase has no action, button is disabled
  action: "install" | "pause" | "play" | "stop" | null;
}

// one table, so a new phase cannot silently fall through to a dead button
const PHASE_SPEC: Record<Phase, Spec> = {
  NEEDS_COMPONENTS: { label: "Install", tone: "primary", action: "install" },
  NEEDS_GAME: { label: "Download", tone: "primary", action: "install" },
  INSTALLING_COMPONENTS: { label: "Installing", tone: "idle", action: null },
  DOWNLOADING: { label: "Pause", tone: "idle", action: "pause" },
  PAUSED: { label: "Resume", tone: "primary", action: "install" },
  VERIFYING: { label: "Pause", tone: "idle", action: "pause" },
  READY: { label: "Launch", tone: "primary", action: "play" },
  STARTING: { label: "Starting", tone: "idle", action: null },
  PLAYING: { label: "Stop", tone: "danger", action: "stop" },
  UNINSTALLING: { label: "Removing", tone: "idle", action: null },
  UPDATING: { label: "Updating", tone: "idle", action: null },
  EDITING: { label: "Editing", tone: "idle", action: null },
};

const TONE: Record<Tone, string> = {
  primary:
    "border-amber bg-amber text-amber-ink hover:bg-amber-hi active:bg-amber-dim active:text-ink",
  danger: "border-led-fail text-led-fail hover:bg-led-fail hover:text-ink",
  idle: "border-hair-lit text-ink-3",
};

export function LaunchButton({
  phase,
  sub,
  onAction,
}: {
  phase: Phase;
  // secondary line, e.g. "15.1 GiB" or the elapsed timer
  sub?: string;
  onAction: (action: NonNullable<Spec["action"]>) => void;
}) {
  const spec = PHASE_SPEC[phase];
  const disabled = spec.action === null;

  return (
    <button
      disabled={disabled}
      onClick={() => spec.action && onAction(spec.action)}
      className={`group flex h-[60px] w-[248px] items-center justify-center gap-3 border transition-all duration-200 ${TONE[spec.tone]} ${
        disabled ? "cursor-default" : "hover:-translate-y-px active:translate-y-0"
      } ${spec.tone === "primary" ? "glow-breathe" : ""}`}
    >
      {spec.action === "play" && (
        <svg
          width="13"
          height="15"
          viewBox="0 0 16 18"
          aria-hidden
          className="transition-transform duration-200 group-hover:translate-x-0.5"
        >
          <path d="M0 0l16 9-16 9z" fill="currentColor" />
        </svg>
      )}
      <span className="flex flex-col items-start leading-none">
        <span
          className="uppercase"
          style={{ fontSize: "var(--text-big)", letterSpacing: "0.04em" }}
        >
          {spec.label}
        </span>
        {sub && <span className="hud mt-1.5 opacity-70">{sub}</span>}
      </span>
    </button>
  );
}
