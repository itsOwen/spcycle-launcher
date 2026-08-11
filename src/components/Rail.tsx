import type { ServiceState, Services } from "@/lib/ipc";

export const TABS = ["play", "files", "server", "config", "about"] as const;
export type Tab = (typeof TABS)[number];

const LED_ROWS: { key: keyof Services; label: string }[] = [
  { key: "mongo", label: "Mongo" },
  { key: "server", label: "Server" },
  { key: "steam", label: "Steam" },
];

function Led({ state }: { state: ServiceState }) {
  const color =
    state === "failed"
      ? "bg-led-fail"
      : state === "up" || state === "starting"
        ? "bg-led-up"
        : "bg-led-idle";
  return (
    <span
      aria-hidden
      className={`size-1.5 shrink-0 ${color} ${state === "starting" ? "led-pulse" : ""}`}
    />
  );
}

const STATE_TEXT: Record<ServiceState, string> = {
  down: "offline",
  starting: "starting",
  up: "running",
  failed: "failed",
};

export function Rail({
  tab,
  onTab,
  services,
}: {
  tab: Tab;
  onTab: (t: Tab) => void;
  services: Services;
}) {
  return (
    <nav className="flex w-[108px] shrink-0 flex-col border-r border-hair bg-panel">
      {TABS.map((t, i) => {
        const active = t === tab;
        return (
          <button
            key={t}
            onClick={() => onTab(t)}
            aria-current={active ? "page" : undefined}
            style={{ animationDelay: `${i * 60}ms` }}
            className={`hud sweep-in relative py-4.5 text-left transition-colors ${
              active
                ? "bg-amber-wash text-amber"
                : "text-ink-3 hover:bg-panel-2 hover:text-ink-2"
            }`}
          >
            {active && (
              <span className="rule-draw absolute inset-y-0 left-0 w-0.5 origin-top bg-amber" />
            )}
            <span className="pl-5">{t}</span>
          </button>
        );
      })}

      <div className="mt-auto border-t border-hair p-3">
        <ul className="space-y-2">
          {LED_ROWS.map(({ key, label }) => (
            <li key={key} className="flex items-center gap-2">
              <Led state={services[key]} />
              <span
                className="hud text-ink-3"
                title={`${label}: ${STATE_TEXT[services[key]]}`}
              >
                {label}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </nav>
  );
}
