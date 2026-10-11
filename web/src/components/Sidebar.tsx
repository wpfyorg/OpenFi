import { useSnap } from "../context";
import { nodeColor } from "../tokens";
import { NAV } from "../tokens";
import { Icon } from "./icons";
import { Tile } from "./Tile";

/** App sidebar: brand, section nav, and a live per-node CPU list. `open` controls the mobile drawer. */
export function Sidebar({ hash, open, onNavigate }: { hash: string; open: boolean; onNavigate: () => void }) {
  const { snap } = useSnap();
  const nodes = snap?.nodes ?? [];
  return (
    <>
      <aside class={open ? "sidebar open" : "sidebar"}>
        <div class="brand">
          <div class="brand-mark">fi</div>
          <div class="brand-name">
            <strong>OpenFi</strong>
            <span>home &middot; {nodes.length || "–"} nodes</span>
          </div>
        </div>
        <nav class="nav">
          {NAV.map((item) => {
            const active = item.hash === "#/" ? hash === "#/" : hash.startsWith(item.hash);
            return (
              <a href={item.hash} class={active ? "nav-item on" : "nav-item"} onClick={onNavigate} style={{ "--section": item.section }}>
                <Tile icon={item.icon} color={item.section} size={22} solid={active} />
                {item.label}
              </a>
            );
          })}
        </nav>
        <div class="sidebar-spacer" />
        <div class="node-list">
          <span class="node-list-label">Nodes</span>
          {nodes.map((n) => {
            const cpu = Math.min(99, Math.round(n.load[0] * 100));
            return (
              <div class="node-list-row" key={n.name}>
                <span class="node-list-sw" style={{ background: nodeColor(n.name) }} />
                <span class="mono">{n.name}</span>
                <span class="node-list-fill" />
                <span class={`dot ${n.online ? "ok" : "bad"}`} />
                <span class="mono muted">{n.online ? `${cpu}%` : "down"}</span>
              </div>
            );
          })}
          {nodes.length === 0 && <span class="muted" style={{ fontSize: 11 }}>Waiting for data…</span>}
        </div>
        <button type="button" class="sidebar-collapse">
          <Icon name="menu" size={15} />
          Collapse
        </button>
      </aside>
      {open && <div class="sidebar-backdrop" onClick={onNavigate} />}
    </>
  );
}
