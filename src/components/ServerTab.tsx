import { useEffect, useState } from "react";
import * as ipc from "@/lib/ipc";
import type { LogKind, ServiceState, Services } from "@/lib/ipc";
import { Panel, Row } from "./kit";

const LOGS: LogKind[] = ["launcher", "mongod", "server", "loader", "game"];

const STATE_TEXT: Record<ServiceState, string> = {
  down: "offline",
  starting: "starting",
  up: "running",
  failed: "failed",
};

const STATE_COLOR: Record<ServiceState, string> = {
  down: "text-ink-3",
  starting: "text-amber",
  up: "text-amber",
  failed: "text-led-fail",
};

export function ServerTab({ services }: { services: Services }) {
  const [which, setWhich] = useState<LogKind>("launcher");
  const [text, setText] = useState("");

  useEffect(() => {
    let live = true;
    const load = () =>
      ipc
        .logTail(which)
        .then((t) => live && setText(t))
        .catch((e) => live && setText(String(e)));
    load();
    // logs only matter while someone is looking at them
    const id = setInterval(load, 2000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, [which]);

  return (
    <div className="flex h-full flex-col gap-4">
      <Panel title="Services">
        <Row
          label="MongoDB"
          value={
            <span className={STATE_COLOR[services.mongo]}>
              {STATE_TEXT[services.mongo]}
            </span>
          }
        />
        <Row
          label="Server API"
          value={
            <span className={STATE_COLOR[services.server]}>
              {STATE_TEXT[services.server]}
            </span>
          }
        />
        <Row
          label="Steam"
          value={
            <span className={STATE_COLOR[services.steam]}>
              {STATE_TEXT[services.steam]}
            </span>
          }
        />
      </Panel>

      <section className="flex min-h-0 flex-1 flex-col border border-hair bg-panel">
        <header className="flex shrink-0 items-center gap-1 border-b border-hair px-2 py-2">
          <span className="hud mr-2 pl-2 text-ink-2">Log</span>
          {LOGS.map((l) => (
            <button
              key={l}
              onClick={() => setWhich(l)}
              className={`hud px-2.5 py-1 transition-colors ${
                which === l
                  ? "bg-amber-wash text-amber"
                  : "text-ink-3 hover:bg-panel-2 hover:text-ink-2"
              }`}
            >
              {l}
            </button>
          ))}
        </header>
        <pre className="min-h-0 flex-1 overflow-auto bg-panel-2 p-3 text-[0.75rem] leading-relaxed text-ink-2 select-text">
          {text || "(empty)"}
        </pre>
      </section>
    </div>
  );
}
