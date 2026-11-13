const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function bytes(n: number): string {
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit === 0 ? `${n} B` : `${value.toFixed(value < 10 ? 1 : 0)} ${UNITS[unit]}`;
}

export function duration(secs: number): string {
  if (secs < 60) return "less than a minute left";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `about ${minutes} min left`;
  const hours = Math.floor(minutes / 60);
  return `about ${hours} h ${minutes % 60} min left`;
}

const compact = new Intl.NumberFormat("en", { notation: "compact" });

/** A count short enough for a badge: exact up to 999, then 1K, 1.2K, 12K, 1.2M. */
export function compactCount(n: number): string {
  return n < 1000 ? String(n) : compact.format(n);
}

/** A count with thousands separators, e.g. "12,345". */
export function exactCount(n: number): string {
  return n.toLocaleString("en");
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Shorten a long path in the middle, keeping the drive/root and the last segments. */
export function shortPath(path: string, max = 44): string {
  if (path.length <= max) return path;
  const sep = path.includes("\\") ? "\\" : "/";
  const parts = path.split(sep);
  let tail = parts.pop() ?? "";
  while (parts.length > 1 && tail.length + parts[parts.length - 1].length + 1 < max - 6) {
    tail = `${parts.pop()}${sep}${tail}`;
  }
  return `${parts[0]}${sep}…${sep}${tail}`;
}

/** First and last characters of a long id, e.g. "a9cf…b9b4". */
export function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 4)}…${id.slice(-4)}` : id;
}
