// Pure helpers over the qos.tiers fair-use ladder (see contractExt.ts), shared by Usage (progress
// toward the next tier per device) and Qos (the full ladder with client counts).
import type { QosTier } from "./contractExt";

export interface TierProgress { tier: QosTier | null; next: QosTier | null; usedGb: number; frac: number | null }

/** The tier a usage total (bytes) currently falls in, by ascending min_gb. */
export function tierFor(bytes: number, tiers: QosTier[]): QosTier | null {
  const gb = bytes / 1e9;
  const sorted = [...tiers].sort((a, b) => a.min_gb - b.min_gb);
  let hit: QosTier | null = null;
  for (const t of sorted) if (gb >= t.min_gb) hit = t; else break;
  return hit ?? sorted[0] ?? null;
}

/** Progress within the current tier toward its ceiling (or the next tier's floor). Null frac when
 *  the ladder has no real min/max data (the contractExt fallback) or the tier is open-ended. */
export function tierProgress(bytes: number, tiers: QosTier[]): TierProgress {
  const usedGb = bytes / 1e9;
  const sorted = [...tiers].sort((a, b) => a.min_gb - b.min_gb);
  const tier = tierFor(bytes, tiers);
  if (!tier) return { tier: null, next: null, usedGb, frac: null };
  const idx = sorted.findIndex((t) => t.id === tier.id);
  const next = idx >= 0 ? sorted[idx + 1] ?? null : null;
  const ceiling = tier.max_gb ?? next?.min_gb ?? null;
  if (ceiling == null || ceiling <= tier.min_gb) return { tier, next, usedGb, frac: null };
  const frac = Math.max(0, Math.min(1, (usedGb - tier.min_gb) / (ceiling - tier.min_gb)));
  return { tier, next, usedGb, frac };
}
