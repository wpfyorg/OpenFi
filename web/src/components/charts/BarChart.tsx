export interface BarSegment { value: number; color: string }
export interface BarGroup { label: string; segments: BarSegment[] }

/** Stacked or grouped bar chart. Pure CSS/SVG, no chart lib — fine for the coarse bars this app needs. */
export function BarChart({
  groups, mode = "stacked", height = 160, valueFmt = String,
}: { groups: BarGroup[]; mode?: "stacked" | "grouped"; height?: number; valueFmt?: (v: number) => string }) {
  const totals = groups.map((g) => (mode === "stacked" ? g.segments.reduce((a, s) => a + s.value, 0) : Math.max(0, ...g.segments.map((s) => s.value))));
  const max = Math.max(1, ...totals);
  return (
    <div class="bar-chart" style={{ height }}>
      {groups.map((g, gi) => (
        <div class="bar-slot" title={`${g.label}: ${valueFmt(totals[gi])}`}>
          <div class={mode === "stacked" ? "bar-stack" : "bar-group"}>
            {g.segments.map((s) => (
              <span
                class="bar-seg"
                style={{ height: `${(s.value / max) * 100}%`, background: s.color }}
              />
            ))}
          </div>
          <span class="bar-label">{g.label}</span>
        </div>
      ))}
    </div>
  );
}
