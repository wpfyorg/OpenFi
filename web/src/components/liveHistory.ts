// In-memory ring buffer built from the live Snapshot stream. The contract has no per-node or
// per-client history endpoint (only WAN-level GET /history), so pages that want a client
// distribution or top-talkers-over-time chart accumulate their own samples while mounted. This
// resets on navigation/reload — callers should label charts built from it accordingly.
import { useEffect, useRef, useState } from "preact/hooks";
import type { Snapshot } from "../types";

export interface LiveHistoryPoint { ts: number; values: Record<string, number> }

export function useLiveHistory(
  snap: Snapshot | null,
  extract: (snap: Snapshot) => Record<string, number>,
  cap = 180,
): LiveHistoryPoint[] {
  const [points, setPoints] = useState<LiveHistoryPoint[]>([]);
  const lastTs = useRef<number | null>(null);
  const extractRef = useRef(extract);
  extractRef.current = extract;

  useEffect(() => {
    if (!snap) return;
    if (lastTs.current === snap.ts) return;
    lastTs.current = snap.ts;
    setPoints((prev) => {
      const next = [...prev, { ts: snap.ts, values: extractRef.current(snap) }];
      return next.length > cap ? next.slice(next.length - cap) : next;
    });
  }, [snap, cap]);

  return points;
}

/** Pull one series (by key) out of a LiveHistoryPoint[], defaulting missing samples to 0. */
export function seriesFor(points: LiveHistoryPoint[], key: string): number[] {
  return points.map((p) => p.values[key] ?? 0);
}

/** Hour:minute x-axis formatter — a live ring buffer usually spans minutes, not the hours AreaChart's
 *  default (hour-only) axis assumes, so ticks would otherwise repeat the same label. */
export function liveHistoryXFmt(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
