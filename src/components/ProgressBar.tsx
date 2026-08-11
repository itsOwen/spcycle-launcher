import { bytes, eta, percent, rate } from "@/lib/format";

export interface Progress {
  done: number;
  // 0 means indeterminate, no total known yet
  total: number;
  label: string;
  bytesPerSecond: number;
  secondsLeft: number | null;
}

export function ProgressBar({
  progress,
  pausable,
  onPause,
}: {
  progress: Progress;
  pausable: boolean;
  onPause: () => void;
}) {
  const { done, total, label } = progress;
  const indeterminate = total <= 0;
  const pct = percent(done, total);

  return (
    <section className="border border-hair bg-panel p-4">
      <div className="flex items-baseline gap-3">
        <span className="hud text-amber">{label || "Working"}</span>
        <span className="ml-auto tabular-nums text-ink-2">
          {indeterminate ? "—" : `${pct.toFixed(1)}%`}
        </span>
      </div>

      <div
        className="mt-3 h-1.5 w-full overflow-hidden bg-panel-3"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={indeterminate ? undefined : Math.round(pct)}
        aria-label={label || "Progress"}
      >
        {indeterminate ? (
          <div className="progress-indeterminate h-full bg-amber" />
        ) : (
          <div
            className="h-full bg-amber transition-[width] duration-200"
            style={{ width: `${pct}%` }}
          />
        )}
      </div>

      <div className="mt-3 flex items-center gap-4 text-ink-3">
        <span className="hud tabular-nums">
          {indeterminate ? bytes(done) : `${bytes(done)} / ${bytes(total)}`}
        </span>
        <span className="hud tabular-nums">{rate(progress.bytesPerSecond)}</span>
        <span className="hud tabular-nums">{eta(progress.secondsLeft)} left</span>

        {pausable && (
          <button
            onClick={onPause}
            className="hud ml-auto border border-hair-lit px-3 py-1.5 text-ink-2 transition-colors hover:border-amber hover:text-amber"
          >
            Pause
          </button>
        )}
      </div>
    </section>
  );
}
