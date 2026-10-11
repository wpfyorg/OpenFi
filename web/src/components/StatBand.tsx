import type { ComponentChildren } from "preact";
import { Tile } from "./Tile";

export interface StatProps {
  label: string; value: ComponentChildren; sub?: string; icon?: string; color?: string;
}

/** One cell of a StatBand: label, big value, optional caption. */
export function Stat({ label, value, sub, icon, color = "var(--text-2)" }: StatProps) {
  return (
    <div class="stat">
      <div class="stat-head">
        {icon && <Tile icon={icon} color={color} size={24} />}
        <span class="stat-label">{label}</span>
      </div>
      <div class="stat-value">{value}</div>
      {sub && <div class="stat-sub">{sub}</div>}
    </div>
  );
}

/** Row of Stat cells on one surface, separated by hairlines — the KPI strip at the top of a page. */
export function StatBand({ children }: { children: ComponentChildren }) {
  return <div class="stat-band">{children}</div>;
}
