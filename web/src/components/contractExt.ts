// Small accessors over contract fields that pages read defensively (a stale backend may omit them).
import type { Qos, QosTier, Wan } from "../types";

export type { QosTier };

export function wanWindow(wan: Wan): { rx: number | null; tx: number | null } {
  return {
    rx: typeof wan.window_rx_bytes === "number" ? wan.window_rx_bytes : null,
    tx: typeof wan.window_tx_bytes === "number" ? wan.window_tx_bytes : null,
  };
}

export function qosTiers(qos: Qos): QosTier[] {
  if (qos.tiers?.length) return qos.tiers;
  // No ladder from the backend: group clients by tier id so the page still shows something,
  // with thresholds and rates unknown rather than guessed.
  const byTier = new Map<string, number>();
  for (const c of qos.clients) byTier.set(c.tier, (byTier.get(c.tier) ?? 0) + 1);
  return [...byTier.entries()].map(([id, clients]) => ({ id, label: id, min_gb: 0, max_gb: null, rate_mbit: null, clients }));
}
