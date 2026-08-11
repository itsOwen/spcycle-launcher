import { useEffect, useState } from "react";
import boot from "@/assets/boot.gif";

// held this long so it reads as deliberate, not a flash on a fast machine
const MIN_MS = 3400;
const FADE_MS = 520;

export function BootSplash({ ready, onDone }: { ready: boolean; onDone: () => void }) {
  const [elapsed, setElapsed] = useState(false);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    const id = setTimeout(() => setElapsed(true), MIN_MS);
    return () => clearTimeout(id);
  }, []);

  // `leaving` is deliberately not a dependency: it would re-run this effect and
  // cancel the timeout that unmounts us, stranding an invisible overlay.
  useEffect(() => {
    if (!elapsed || !ready) return;
    setLeaving(true);
    const id = setTimeout(onDone, FADE_MS);
    return () => clearTimeout(id);
  }, [elapsed, ready, onDone]);

  return (
    <div
      className={`fixed inset-0 z-[100] flex flex-col items-center justify-center bg-void transition-opacity duration-500 ${
        leaving ? "pointer-events-none opacity-0" : "opacity-100"
      }`}
    >
      <img
        src={boot}
        alt=""
        width={200}
        className="rise mb-8 border border-hair"
        style={{ animationDelay: "80ms" }}
      />

      <div className="rise flex items-baseline gap-3" style={{ animationDelay: "220ms" }}>
        <span
          className="uppercase text-amber"
          style={{ fontSize: "1.5rem", letterSpacing: "0.22em" }}
        >
          SPCycle
        </span>
        <span className="hud text-ink-3">Fortuna III</span>
      </div>

      <div className="mt-6 h-px w-[260px] overflow-hidden bg-hair">
        <div
          className="boot-bar h-full bg-amber"
          style={{ animationDuration: `${MIN_MS}ms` }}
        />
      </div>

      <p className="hud rise mt-4 text-ink-3" style={{ animationDelay: "360ms" }}>
        {ready ? "Ready" : "Checking your install"}
      </p>
    </div>
  );
}
