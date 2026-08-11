const UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

// 39511307208 -> "36.80 GiB"
export function bytes(n: number, digits = 2): string {
  if (!Number.isFinite(n) || n <= 0) return `0 ${UNITS[0]}`;
  let i = 0;
  let v = n;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  // whole bytes and KiB never want decimals
  return `${v.toFixed(i < 2 ? 0 : digits)} ${UNITS[i]}`;
}

// bytes per second -> "41.2 MiB/s"
export function rate(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "—";
  return `${bytes(bytesPerSecond, 1)}/s`;
}

// seconds -> "14:22" or "1:04:11". null when it cannot be known yet.
export function eta(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) return "—";
  const s = Math.round(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (x: number) => String(x).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}

// elapsed wall clock since a timestamp, for the playing readout
export function elapsed(sinceMs: number, nowMs: number): string {
  return eta(Math.max(0, (nowMs - sinceMs) / 1000));
}

export function percent(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.max(0, (done / total) * 100));
}
