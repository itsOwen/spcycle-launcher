import type { ReactNode } from "react";

// shared shell primitives, so the four tabs don't each reinvent the markup

export function Panel({
  title,
  children,
  actions,
}: {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="border border-hair bg-panel">
      <header className="flex items-center gap-3 border-b border-hair px-4 py-2.5">
        <span className="hud text-ink-2">{title}</span>
        {actions && <div className="ml-auto flex gap-2">{actions}</div>}
      </header>
      <div className="p-4">{children}</div>
    </section>
  );
}

// a label/value line. values are tabular so columns of numbers line up.
export function Row({
  label,
  value,
  mono = true,
  title,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
  title?: string;
}) {
  return (
    <div className="flex items-baseline gap-4 py-1.5">
      <span className="hud w-[120px] shrink-0 text-ink-3">{label}</span>
      <span
        className={`min-w-0 truncate text-ink ${mono ? "tabular-nums" : ""}`}
        title={title ?? (typeof value === "string" ? value : undefined)}
      >
        {value}
      </span>
    </div>
  );
}

export function Btn({
  children,
  onClick,
  disabled,
  tone = "normal",
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  tone?: "normal" | "danger";
}) {
  const style =
    tone === "danger"
      ? "border-hair-lit text-ink-2 hover:border-led-fail hover:text-led-fail"
      : "border-hair-lit text-ink-2 hover:border-amber hover:text-amber";
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`hud border px-3 py-1.5 transition-colors ${style} disabled:border-hair disabled:text-ink-3 disabled:hover:border-hair disabled:hover:text-ink-3`}
    >
      {children}
    </button>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="text-ink-3">{children}</p>;
}
