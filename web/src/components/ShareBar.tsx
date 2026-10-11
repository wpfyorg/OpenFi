/** Thin proportional fill bar (0-1 fraction) — usage shares, meters. */
export function ShareBar({ frac, color = "var(--section)", width }: { frac: number; color?: string; width?: number | string }) {
  const pct = `${Math.max(0, Math.min(1, frac)) * 100}%`;
  return (
    <span class="share-bar" style={{ width }}>
      <i style={{ width: pct, background: color }} />
    </span>
  );
}
