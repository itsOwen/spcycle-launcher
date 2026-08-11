import type { Check, Preflight } from "@/lib/ipc";
import { useDialog } from "@/hooks/useDialog";
import { Btn } from "./kit";

// copy to clipboard without the plugin: nothing else in the app needs it
async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const el = document.createElement("textarea");
    el.value = text;
    el.style.position = "fixed";
    el.style.opacity = "0";
    document.body.appendChild(el);
    el.select();
    document.execCommand("copy");
    el.remove();
  }
}

function Row({ check }: { check: Check }) {
  const blocking = check.severity === "blocking";
  return (
    <li
      className={`border-l-2 bg-panel-2 p-3 ${blocking ? "border-l-led-fail" : "border-l-amber"}`}
    >
      <div className="flex items-baseline gap-3">
        <span className="text-ink">{check.title}</span>
        <span className={`hud ml-auto ${blocking ? "text-led-fail" : "text-amber"}`}>
          {blocking ? "required" : "recommended"}
        </span>
      </div>
      <p className="mt-1.5 leading-relaxed text-ink-2">{check.impact}</p>
      {check.install && (
        <div className="mt-2 flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate bg-void px-2 py-1.5 text-ink-2 select-text">
            {check.install}
          </code>
          <Btn onClick={() => check.install && copy(check.install)}>Copy</Btn>
        </div>
      )}
    </li>
  );
}

// split because the emptiness check precedes useDialog, and a hook cannot sit
// behind an early return
export function PreflightDialog({
  report,
  onDismiss,
}: {
  report: Preflight;
  onDismiss: () => void;
}) {
  const failing = report.checks.filter((c) => !c.ok);
  if (failing.length === 0) return null;
  return <PreflightModal report={report} onDismiss={onDismiss} failing={failing} />;
}

function PreflightModal({
  report,
  onDismiss,
  failing,
}: {
  report: Preflight;
  onDismiss: () => void;
  failing: Check[];
}) {
  const dialog = useDialog(onDismiss);

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center bg-void/80 p-8">
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="preflight-title"
        className="flex max-h-full w-[560px] flex-col border border-hair bg-panel"
      >
        <header className="border-b border-hair px-5 py-3">
          <h2 id="preflight-title" className="hud text-ink">
            Before you play
          </h2>
          {report.distro && <p className="hud mt-1 text-ink-3">{report.distro}</p>}
        </header>

        <div className="min-h-0 flex-1 overflow-auto p-5">
          <ul className="space-y-2">
            {failing.map((c) => (
              <Row key={c.id} check={c} />
            ))}
          </ul>
        </div>

        <footer className="flex justify-end gap-2 border-t border-hair px-5 py-3">
          <Btn onClick={onDismiss}>
            {report.hasBlocking ? "Continue anyway" : "Got it"}
          </Btn>
        </footer>
      </div>
    </div>
  );
}
