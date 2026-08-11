import { getCurrentWindow } from "@tauri-apps/api/window";

const win = getCurrentWindow();

export function Titlebar({ version }: { version: string }) {
  return (
    <header
      data-tauri-drag-region
      className="flex h-10 shrink-0 items-center gap-3 border-b border-hair bg-panel px-3"
    >
      {/* the mark: a cut corner, echoing the 0px-radius language */}
      <span
        aria-hidden
        className="size-2.5 bg-amber"
        style={{ clipPath: "polygon(0 0, 100% 0, 100% 100%)" }}
      />
      <span className="hud text-ink">SPCycle</span>
      <span className="hud text-ink-3">{version ? `v${version}` : ""}</span>

      <div className="ml-auto flex items-center" data-tauri-drag-region={false}>
        <button
          onClick={() => win.minimize()}
          aria-label="Minimise"
          className="flex h-10 w-11 items-center justify-center text-ink-2 transition-colors hover:bg-panel-3 hover:text-ink"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <rect x="0" y="4.5" width="10" height="1" fill="currentColor" />
          </svg>
        </button>
        <button
          onClick={() => win.close()}
          aria-label="Close"
          className="flex h-10 w-11 items-center justify-center text-ink-2 transition-colors hover:bg-led-fail hover:text-ink"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <path d="M0 0l10 10M10 0L0 10" stroke="currentColor" strokeWidth="1" />
          </svg>
        </button>
      </div>
    </header>
  );
}
