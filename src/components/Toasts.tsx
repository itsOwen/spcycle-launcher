export interface Toast {
  id: number;
  text: string;
  level: 0 | 1 | 2;
}

const EDGE = ["border-l-hair-lit", "border-l-amber", "border-l-led-fail"] as const;

export function Toasts({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: number) => void;
}) {
  if (toasts.length === 0) return null;

  return (
    <div
      className="pointer-events-none absolute top-4 right-4 z-50 flex w-[340px] flex-col gap-2"
      role="status"
      aria-live="polite"
    >
      {/* newest first: the stack hangs off the top edge, so it grows downwards */}
      {[...toasts].reverse().map((t) => (
        <button
          key={t.id}
          onClick={() => onDismiss(t.id)}
          className={`toast-in pointer-events-auto border border-hair ${EDGE[t.level]} border-l-2 bg-panel-2 p-3 text-left text-ink-2 transition-colors select-text hover:text-ink`}
        >
          {t.text}
        </button>
      ))}
    </div>
  );
}
