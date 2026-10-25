// Human-readable sizes and durations.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** Decimal units, like Explorer's drive sizes ("62.5 GB"). */
export function bytes(n: number): string {
  let value = n;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

export function rate(bytesPerSecond: number): string {
  return `${bytes(bytesPerSecond)}/s`;
}

/** "1 h 05 min", "4 min 10 s", "12 s". */
export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const rest = s % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min ${String(rest).padStart(2, "0")} s`;
  return `${rest} s`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
