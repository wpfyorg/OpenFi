/** Tiny inline trend line (no axes) — DNS query volume, 7-day overview strips. Pure SVG, no chart lib. */
export function Sparkline({ values, color = "var(--section)", width = 120, height = 32 }: { values: number[]; color?: string; width?: number; height?: number }) {
  if (values.length < 2) return <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} />;
  const max = Math.max(...values, 1) * 1.1;
  const step = width / (values.length - 1);
  const pts = values.map((v, i) => [i * step, height - (v / max) * height] as const);
  const line = "M" + pts.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" L");
  const area = `${line} L${width},${height} L0,${height} Z`;
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" class="sparkline">
      <path d={area} fill={color} fill-opacity="0.18" stroke="none" />
      <path d={line} fill="none" stroke={color} stroke-width="1.3" stroke-linejoin="round" stroke-linecap="round" />
    </svg>
  );
}
