import type { ComponentChildren } from "preact";
import { Tile } from "./Tile";

/** Page header: solid section tile + title + live status subtitle + optional right-side controls. */
export function PageHeader({ icon, title, subtitle, right }: { icon: string; title: string; subtitle?: string; right?: ComponentChildren }) {
  return (
    <header class="page-head">
      <div class="page-head-title">
        <Tile icon={icon} color="var(--section)" size={40} solid />
        <div class="page-head-titles">
          <h1>{title}</h1>
          {subtitle && <p>{subtitle}</p>}
        </div>
      </div>
      {right && <div class="page-head-right">{right}</div>}
    </header>
  );
}
