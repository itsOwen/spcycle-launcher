import { useEffect, useRef } from "react";

const FOCUSABLE = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

// escape to leave, focus moved in on open and handed back on close, and tab kept
// inside. without this a modal is only a div drawn on top: the page behind stays
// reachable, and a keyboard user has to tab the whole app to reach its buttons.
//
// returns the ref to put on the dialog element itself, not the backdrop.
export function useDialog(onDismiss: () => void) {
  const ref = useRef<HTMLDivElement>(null);

  // read through a ref so a caller passing an inline closure does not re-run the
  // effect on every render and steal focus back each time
  const dismiss = useRef(onDismiss);
  dismiss.current = onDismiss;

  useEffect(() => {
    const node = ref.current;
    const opener = document.activeElement as HTMLElement | null;
    const items = () => Array.from(node?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);

    items()[0]?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        dismiss.current();
        return;
      }
      if (e.key !== "Tab") return;

      const focusable = items();
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;

      if (e.shiftKey && (active === first || !node?.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      opener?.focus?.();
    };
  }, []);

  return ref;
}
