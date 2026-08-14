

const BTN =
  "flex h-7 w-8 items-center justify-center transition-colors disabled:text-hair-lit disabled:hover:bg-transparent";

export function BackdropControls({
  video,
  sound,
  volume,
  ready,
  blocked,
  onVideo,
  onSound,
  onVolume,
  onBlocked,
}: {
  video: boolean;
  sound: boolean;
  volume: number;
  ready: boolean;
  // why the video cannot play on this machine, or null when it can
  blocked: string | null;
  onVideo: (on: boolean) => void;
  onSound: (on: boolean) => void;
  onVolume: (v: number) => void;
  onBlocked: (reason: string) => void;
}) {
  const live = ready && video && !blocked;

  return (
    <div className="group flex shrink-0 items-center border border-hair bg-panel/70 backdrop-blur-sm transition-colors hover:border-hair-lit">
      {/* the cut corner from the app mark, so this reads as ours */}
      <span
        aria-hidden
        className="ml-1.5 size-1.5 shrink-0 bg-amber-dim transition-colors group-hover:bg-amber"
        style={{ clipPath: "polygon(0 0, 100% 0, 100% 100%)" }}
      />

      {/* not disabled when blocked: a dead button explains nothing, so it says why */}
      <button
        onClick={() => (blocked ? onBlocked(blocked) : onVideo(!video))}
        disabled={!ready}
        aria-label={video ? "Turn the video backdrop off" : "Turn it on"}
        aria-pressed={video && !blocked}
        title={blocked ?? (video ? "Video backdrop on" : "Video backdrop off")}
        className={`${BTN} ml-0.5 hover:bg-panel-3 ${
          video && !blocked ? "text-amber" : "text-ink-3 hover:text-ink"
        }`}
      >
        <svg width="14" height="12" viewBox="0 0 14 12" aria-hidden>
          <rect x="0.5" y="1.5" width="9" height="9" stroke="currentColor" fill="none" />
          <path d="M9.5 5L13.5 2.5v7L9.5 7z" fill="currentColor" />
          {(!video || blocked) && (
            <path d="M0 12L14 0" stroke="currentColor" strokeWidth="1.2" />
          )}
        </svg>
      </button>

      <button
        onClick={() => onSound(!sound)}
        disabled={!live}
        aria-label={sound ? "Mute the backdrop" : "Unmute it"}
        aria-pressed={sound}
        title={sound ? "Backdrop sound on" : "Backdrop sound off"}
        className={`${BTN} hover:bg-panel-3 ${
          sound && video ? "text-amber" : "text-ink-3 hover:text-ink"
        }`}
      >
        <svg width="14" height="12" viewBox="0 0 14 12" aria-hidden>
          <path d="M1 4.5h2.5L7 1.5v9L3.5 7.5H1z" fill="currentColor" />
          {sound ? (
            <path d="M9.5 3.5a4 4 0 010 5M11.5 2a6.5 6.5 0 010 8" stroke="currentColor" fill="none" />
          ) : (
            <path d="M9.5 4l4 4M13.5 4l-4 4" stroke="currentColor" strokeWidth="1.2" />
          )}
        </svg>
      </button>

      {/* stays out of the way until wanted; focus-within so it is reachable by keyboard */}
      <div className="w-0 overflow-hidden transition-[width] duration-200 ease-out group-hover:w-[5.5rem] group-focus-within:w-[5.5rem]">
        <div className="flex items-center gap-2 pl-1 pr-2">
          <input
            className="vol"
            type="range"
            min={0}
            max={100}
            step={5}
            value={volume}
            disabled={!live || !sound}
            aria-label="Backdrop volume"
            onChange={(e) => onVolume(Number(e.target.value))}
          />
          <span className="hud w-6 shrink-0 tabular-nums text-right text-ink-3">
            {volume}
          </span>
        </div>
      </div>
    </div>
  );
}
