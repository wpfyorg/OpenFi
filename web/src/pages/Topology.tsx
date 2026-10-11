// Mesh map: an SVG graph built from Snapshot.nodes[].links (no hard-coded positions), with
// zoom/pan, per-node client clusters, link-quality colouring, and a NodeCard side panel.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { NodeCard } from "../components/NodeCard";
import { ShellDrawer } from "../components/ShellDrawer";
import { Chip } from "../components/Chip";
import { Empty } from "../components/States";
import { Icon } from "../components/icons";
import { Panel } from "../components/Panel";
import { useSnap } from "../context";
import { nodeColor } from "../tokens";
import type { Link, Node } from "../types";

const SECTION = "#22B8A6";
const W = 1000;
const H = 460;
const BOX_W = 176;
const BOX_H = 58;
const ONT_Y = 40;
const GW_Y = 190;
const AP_Y = 360;
const BAND_COLORS: Record<string, string> = { "2.4": "#7AA2FF", "5": "#2AA7E0" };

interface Pos { x: number; y: number }

export default function Topology() {
  const { snap, connected } = useSnap();
  const [selected, setSelected] = useState<string | null>(null);
  const [shellNode, setShellNode] = useState<string | null>(null);

  const gateway = useMemo(() => snap?.nodes.find((n) => n.role === "gateway") ?? null, [snap]);
  const aps = useMemo(() => snap?.nodes.filter((n) => n.role !== "gateway") ?? [], [snap]);

  // Default selection: the gateway (the hub of the graph), once data arrives.
  useEffect(() => {
    if (selected != null || !snap) return;
    setSelected(gateway?.name ?? snap.nodes[0]?.name ?? null);
  }, [snap, gateway, selected]);

  if (!snap) {
    return (
      <div class="page" style={{ "--section": SECTION }}>
        <PageHead connected={connected} nodeCount={0} onlineCount={0} />
        <Panel><Empty icon="network" title="Waiting for the first poll…" /></Panel>
      </div>
    );
  }

  const online = snap.nodes.filter((n) => n.online).length;
  const selectedNode = snap.nodes.find((n) => n.name === selected) ?? null;

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHead connected={connected} nodeCount={snap.nodes.length} onlineCount={online} />
      <div class="row topo-row">
        <Panel class="panel-topo">
          <TopologyGraph
            gateway={gateway}
            aps={aps}
            wanIp={snap.wan.ip}
            wanGateway={snap.wan.gateway}
            selected={selected}
            onSelect={setSelected}
          />
        </Panel>
        {selectedNode && (
          <NodeCard node={selectedNode} wanIp={snap.wan.ip} onOpenShell={setShellNode} />
        )}
      </div>
      {shellNode && <ShellDrawer node={shellNode} onClose={() => setShellNode(null)} />}
    </div>
  );
}

function PageHead({ connected, nodeCount, onlineCount }: { connected: boolean; nodeCount: number; onlineCount: number }) {
  const subtitle = nodeCount === 0
    ? "Mesh map and link quality"
    : `${onlineCount}/${nodeCount} node${nodeCount === 1 ? "" : "s"} online. Select a node to see its card.`;
  return (
    <header class="page-head">
      <div class="page-head-title">
        <div class="tile" style={{ width: 40, height: 40, borderRadius: 11, background: "var(--section)" }}>
          <Icon name="network" size={24} color="#fff" />
        </div>
        <div class="page-head-titles">
          <h1>Topology</h1>
          <p>{subtitle}</p>
        </div>
      </div>
      <div class="page-head-right">
        <Chip label={connected ? "Live" : "Reconnecting…"} color={connected ? "var(--ok)" : "var(--warn)"} />
      </div>
    </header>
  );
}

interface ViewState { cx: number; cy: number; s: number }
const BASE_VIEW: ViewState = { cx: W / 2, cy: H / 2, s: 1 };

function TopologyGraph({
  gateway, aps, wanIp, wanGateway, selected, onSelect,
}: { gateway: Node | null; aps: Node[]; wanIp: string | null; wanGateway: string | null; selected: string | null; onSelect: (n: string) => void }) {
  const svgRef = useRef<SVGSVGElement>(null);
  const [view, setView] = useState<ViewState>(BASE_VIEW);
  const drag = useRef<{ active: boolean; startX: number; startY: number; origCx: number; origCy: number } | null>(null);

  const gwPos: Pos = { x: W / 2, y: GW_Y };
  const ontPos: Pos = { x: W / 2, y: ONT_Y };
  const apPositions = new Map<string, Pos>();
  aps.forEach((ap, i) => {
    const x = aps.length === 1 ? W / 2 : 140 + (i * (W - 280)) / (aps.length - 1);
    apPositions.set(ap.name, { x, y: AP_Y });
  });

  function clampZoom(s: number) { return Math.max(0.5, Math.min(3, s)); }
  function zoomBy(factor: number) { setView((v) => ({ ...v, s: clampZoom(v.s * factor) })); }
  function fit() { setView(BASE_VIEW); }

  function viewBox() {
    const w = W / view.s, h = H / view.s;
    return { x: view.cx - w / 2, y: view.cy - h / 2, w, h };
  }
  const vb = viewBox();

  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey) return; // trackpad pinch sends ctrlKey+wheel; plain scroll is left to the page
    e.preventDefault();
    zoomBy(e.deltaY < 0 ? 1.1 : 0.9);
  }

  function onPointerDown(e: PointerEvent) {
    if ((e.target as Element).closest("[data-node-box]")) return;
    drag.current = { active: true, startX: e.clientX, startY: e.clientY, origCx: view.cx, origCy: view.cy };
    (e.target as Element).setPointerCapture?.(e.pointerId);
  }
  function onPointerMove(e: PointerEvent) {
    const d = drag.current;
    if (!d?.active || !svgRef.current) return;
    const rect = svgRef.current.getBoundingClientRect();
    const scale = (W / view.s) / rect.width;
    setView((v) => ({ ...v, cx: d.origCx - (e.clientX - d.startX) * scale, cy: d.origCy - (e.clientY - d.startY) * scale }));
  }
  function onPointerUp() { if (drag.current) drag.current.active = false; }

  return (
    <div class="topo-wrap">
      <div class="topo-toolbar">
        <span class="muted small">Drag to pan, ctrl+scroll to zoom.</span>
        <div class="topo-zoom-controls">
          <button type="button" class="icon-btn" onClick={() => zoomBy(1.25)} aria-label="Zoom in"><Icon name="zoom-in" size={15} /></button>
          <button type="button" class="icon-btn" onClick={() => zoomBy(0.8)} aria-label="Zoom out"><Icon name="zoom-out" size={15} /></button>
          <button type="button" class="icon-btn" onClick={fit} aria-label="Fit to view"><Icon name="crosshair" size={15} /></button>
        </div>
      </div>
      <svg
        ref={svgRef}
        class="topo-svg"
        viewBox={`${vb.x} ${vb.y} ${vb.w} ${vb.h}`}
        onWheel={onWheel}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerLeave={onPointerUp}
        role="img"
        aria-label="Network topology graph"
      >
        {/* WAN uplink: internet -> gateway */}
        {gateway && <LinkLine from={ontPos} to={gwPos} color="var(--text-3)" dashed={!wanIp} label={wanIp ? `WAN · ${wanIp}` : "WAN down"} />}

        {/* gateway -> each AP: one line per non-wan link the AP reports */}
        {gateway && aps.map((ap) => {
          const pos = apPositions.get(ap.name)!;
          const links = ap.links.filter((l) => l.kind !== "wan");
          if (links.length === 0) return <LinkLine key={ap.name} from={gwPos} to={pos} color="var(--text-3)" dashed={!ap.online} label="no link" />;
          return links.map((l, i) => (
            <LinkLine
              key={`${ap.name}-${i}`}
              from={offset(gwPos, i, links.length)}
              to={offset(pos, i, links.length)}
              color={ap.online ? qualityColor(l) : "var(--danger)"}
              dashed={!ap.online}
              label={linkLabel(l)}
            />
          ));
        })}

        {/* nodes, drawn on top of links */}
        <NodeBox pos={ontPos} icon="globe" name="Internet" sub={wanGateway ? `${wanGateway} · upstream` : "no upstream"} color="#3a4452" selected={false} onSelect={undefined} />
        {gateway && (
          <NodeBox
            pos={gwPos} icon="router" name={gateway.name} sub={`${gateway.host} · gateway`}
            color={nodeColor(gateway.name)} online={gateway.online} clients={clientTotal(gateway)}
            selected={selected === gateway.name} onSelect={() => onSelect(gateway.name)}
          />
        )}
        {aps.map((ap) => (
          <NodeBox
            key={ap.name} pos={apPositions.get(ap.name)!} icon="wifi" name={ap.name} sub={`${ap.host} · ${roleLabel(ap)}`}
            color={nodeColor(ap.name)} online={ap.online} clients={clientTotal(ap)}
            selected={selected === ap.name} onSelect={() => onSelect(ap.name)}
          />
        ))}
      </svg>
      <div class="legend">
        <span class="legend-item"><i style={{ background: "var(--ok)" }} />mesh (strong)</span>
        <span class="legend-item"><i style={{ background: "var(--yellow)" }} />mesh (fair)</span>
        <span class="legend-item"><i style={{ background: "var(--danger)" }} />mesh (weak) / offline</span>
        <span class="legend-item"><i style={{ background: "var(--text-3)" }} />ethernet / WAN</span>
      </div>
    </div>
  );
}

/** Offsets the endpoint of one of several parallel links between the same two boxes, so they don't overlap. */
function offset(p: Pos, i: number, total: number): Pos {
  if (total <= 1) return p;
  const spread = 20;
  const dx = -spread / 2 + (i * spread) / (total - 1);
  return { x: p.x + dx, y: p.y };
}

function clientTotal(n: Node): number { return n.radios.reduce((a, r) => a + r.clients, 0); }

function roleLabel(n: Node): string {
  const wired = n.links.some((l) => l.kind === "wired");
  return wired ? "wired + mesh" : "mesh only";
}

function qualityColor(l: Link): string {
  if (l.kind === "wired") return "var(--text-3)";
  return l.quality === "strong" ? "var(--ok)" : l.quality === "fair" ? "var(--yellow)" : "var(--danger)";
}

function linkLabel(l: Link): string {
  if (l.kind === "wired") return l.rx_mbps != null ? `wired · ${Math.round(l.rx_mbps)} Mbps` : "wired";
  const parts: string[] = [];
  if (l.signal_dbm != null) parts.push(`${l.signal_dbm} dBm`);
  if (l.rx_mbps != null) parts.push(`${Math.round(l.rx_mbps)} Mbps`);
  return parts.join(" · ") || "mesh";
}

function LinkLine({ from, to, color, dashed, label }: { from: Pos; to: Pos; color: string; dashed: boolean; label: string }) {
  const midX = (from.x + to.x) / 2;
  const midY = (from.y + to.y) / 2;
  return (
    <g>
      <line x1={from.x} y1={from.y} x2={to.x} y2={to.y} stroke={color} stroke-width={2.5} stroke-dasharray={dashed ? "6 5" : undefined} />
      <foreignObject x={midX - 70} y={midY - 11} width={140} height={22}>
        <div class="topo-link-label mono" style={{ borderColor: color, color }}>{label}</div>
      </foreignObject>
    </g>
  );
}

function NodeBox({
  pos, icon, name, sub, color, online = true, clients, selected, onSelect,
}: {
  pos: Pos; icon: string; name: string; sub: string; color: string; online?: boolean; clients?: number;
  selected: boolean; onSelect: (() => void) | undefined;
}) {
  const dim = !online;
  const content = (
    <div class={`topo-node ${selected ? "selected" : ""} ${dim ? "dim" : ""}`} style={{ "--node-color": color }}>
      <span class="topo-node-tile" style={{ background: dim ? "var(--text-3)" : color }}>
        <Icon name={icon} size={16} color="#fff" />
      </span>
      <div class="topo-node-text">
        <span class="mono topo-node-name">{name}</span>
        <span class="topo-node-sub">{sub}</span>
      </div>
      {clients != null && clients > 0 && <span class="chip topo-node-clients">{clients}</span>}
    </div>
  );
  return (
    <foreignObject x={pos.x - BOX_W / 2} y={pos.y - BOX_H / 2} width={BOX_W} height={BOX_H} data-node-box="true">
      {onSelect ? (
        <button type="button" class="topo-node-btn" onClick={onSelect} aria-pressed={selected} aria-label={`Select ${name}`}>
          {content}
        </button>
      ) : content}
    </foreignObject>
  );
}
