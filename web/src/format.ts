export const mbps = (bps: number) => (bps / 1e6).toFixed(1);
export const gb = (bytes: number) => (bytes / 1e9).toFixed(1);

/** Auto-scaled byte total, e.g. for a client's month-to-date usage. */
export function bytes(n: number) {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1)} KB`;
  return `${n} B`;
}

/** Auto-scaled bit rate, for the small "now" rates in the top-talkers table. */
export function bitrate(bps: number) {
  if (bps >= 1e6) return `${(bps / 1e6).toFixed(1)} Mbps`;
  if (bps >= 1e3) return `${(bps / 1e3).toFixed(0)} Kbps`;
  return `${bps} bps`;
}

export function uptime(s: number) {
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

export function pct(n: number, digits = 0) { return `${n.toFixed(digits)}%`; }

export function dbm(n: number | null) { return n == null ? "–" : `${n} dBm`; }

export function celsius(n: number) { return `${Math.round(n)}°`; }

export function relTime(ts: number | null) {
  if (ts == null) return "–";
  const s = Math.max(0, Date.now() / 1000 - ts);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

export function clock(ts: number) {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** Signal-strength colour token: strong/fair/weak thresholds shared with mesh link quality. */
export function signalTone(d: number | null): "strong" | "fair" | "weak" {
  if (d == null) return "weak";
  return d > -60 ? "strong" : d > -72 ? "fair" : "weak";
}
