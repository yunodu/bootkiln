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

/** Rounds time left more coarsely the further away it is, so the estimate reads
 * steadily: 5 s steps under a minute, 10 s under ten minutes, whole minutes beyond. */
export function roundEta(seconds: number): number {
  const s = Math.max(0, seconds);
  if (s < 60) return Math.max(5, Math.ceil(s / 5) * 5);
  if (s < 600) return Math.ceil(s / 10) * 10;
  return Math.ceil(s / 60) * 60;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
