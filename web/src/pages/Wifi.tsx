// WiFi: radios, per-AP client/throughput trends (accumulated client-side from the live stream,
// since the contract has no per-node history endpoint), and mesh backhaul link quality.
import { AreaChart } from "../components/charts/AreaChart";
import { Chip } from "../components/Chip";
import { Empty } from "../components/States";
import { Legend } from "../components/Legend";
import { liveHistoryXFmt, seriesFor, useLiveHistory } from "../components/liveHistory";
import { NodeTag } from "../components/NodeTag";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Stat, StatBand } from "../components/StatBand";
import { Table } from "../components/Table";
import { Tile } from "../components/Tile";
import { useSnap } from "../context";
import { dbm, signalTone } from "../format";
import { nodeColor } from "../tokens";
import type { Link, Node, Snapshot } from "../types";

const SECTION = "#9476F0";

export default function Wifi() {
  const { snap, connected } = useSnap();

  const clientHistory = useLiveHistory(snap, (s) => {
    const out: Record<string, number> = {};
    for (const n of s.nodes) out[n.name] = n.radios.reduce((a, r) => a + r.clients, 0);
    return out;
  });
  const tpHistory = useLiveHistory(snap, (s) => {
    const out: Record<string, number> = {};
    for (const n of s.nodes) out[n.name] = 0;
    const nodeOf = new Map(s.clients.map((c) => [c.mac, c.node]));
    for (const cu of s.usage?.clients ?? []) {
      const node = nodeOf.get(cu.mac);
      if (node != null && node in out) out[node] += (cu.rx_bps + cu.tx_bps) / 1e6;
    }
    return out;
  });

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="wifi" title="WiFi" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="wifi" title="Waiting for the first poll…" /></Panel>
      ) : (
        <>
          <WifiStats snap={snap} history={clientHistory.map((p) => Object.values(p.values).reduce((a, b) => a + b, 0))} />
          <div class="row row-top">
            <ClientDistribution nodes={snap.nodes} history={clientHistory} />
            <Throughput nodes={snap.nodes} history={tpHistory} />
          </div>
          <RadiosPanel nodes={snap.nodes} />
          <MeshPanel nodes={snap.nodes} />
        </>
      )}
    </div>
  );
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  const wifiClients = snap.clients.filter((c) => c.online && !c.wired);
  const apCount = snap.nodes.filter((n) => n.radios.length > 0).length;
  const ssidCount = new Set(snap.nodes.flatMap((n) => n.radios.flatMap((r) => r.ssids))).size;
  return `${apCount} access point${apCount === 1 ? "" : "s"}, ${wifiClients.length} client${wifiClients.length === 1 ? "" : "s"} on ${ssidCount} network${ssidCount === 1 ? "" : "s"}.`;
}

/** Stat band: connected clients, today's peak (since this page opened — no history endpoint exists
 *  for a true 7d/all-time peak), SSIDs, access points, and a strong/fair/weak signal mix bar. */
function WifiStats({ snap, history }: { snap: Snapshot; history: number[] }) {
  const wifiClients = snap.clients.filter((c) => c.online && !c.wired);
  const apNodes = snap.nodes.filter((n) => n.radios.length > 0);
  const totalRadios = apNodes.reduce((a, n) => a + n.radios.length, 0);
  const ssids = [...new Set(snap.nodes.flatMap((n) => n.radios.flatMap((r) => r.ssids)))];
  const peakToday = history.length ? Math.max(wifiClients.length, ...history) : wifiClients.length;

  const mix = { strong: 0, fair: 0, weak: 0 };
  for (const c of wifiClients) mix[signalTone(c.signal_dbm)]++;
  const mixTotal = Math.max(1, wifiClients.length);

  return (
    <StatBand>
      <Stat icon="monitor-smartphone" color="#9476F0" label="Connected clients" value={wifiClients.length}
        sub={`${apNodes.length} access point${apNodes.length === 1 ? "" : "s"}`} />
      <Stat icon="trending-up" color="#3DD68C" label="Peak clients" value={peakToday}
        sub="today, since page opened · 7d/all-time: n/a" />
      <Stat icon="layers" color="#4E8BFF" label="SSIDs" value={ssids.length}
        sub={ssids.slice(0, 2).join(", ") || "None broadcasting"} />
      <Stat icon="router" color="#2CC5C5" label="Access points" value={apNodes.length}
        sub={`${totalRadios} radio${totalRadios === 1 ? "" : "s"} online`} />
      <div class="stat">
        <div class="stat-head"><Tile icon="activity" color="#3DD68C" size={24} /><span class="stat-label">Signal mix</span></div>
        <div class="stat-split">
          <div class="stat-split-cell"><span class="n mono" style={{ color: "var(--ok)" }}>{mix.strong}</span><span class="l">strong</span></div>
          <div class="stat-split-cell"><span class="n mono" style={{ color: "var(--yellow)" }}>{mix.fair}</span><span class="l">fair</span></div>
          <div class="stat-split-cell"><span class="n mono" style={{ color: "var(--danger)" }}>{mix.weak}</span><span class="l">weak</span></div>
        </div>
        <div class="stat-mix-bar">
          <span style={{ width: `${(mix.strong / mixTotal) * 100}%`, background: "var(--ok)" }} />
          <span style={{ width: `${(mix.fair / mixTotal) * 100}%`, background: "var(--yellow)" }} />
          <span style={{ width: `${(mix.weak / mixTotal) * 100}%`, background: "var(--danger)" }} />
        </div>
      </div>
    </StatBand>
  );
}

/** Client counts per AP, stacked over time — accumulated client-side while this page is open. */
function ClientDistribution({ nodes, history }: { nodes: Node[]; history: ReturnType<typeof useLiveHistory> }) {
  const apNodes = nodes.filter((n) => n.radios.length > 0);
  const timestamps = history.map((p) => p.ts);
  const series = apNodes.map((n) => ({ label: n.name, color: nodeColor(n.name), values: seriesFor(history, n.name) }));
  return (
    <Panel title="Client distribution" subtitle="Clients per access point. Starts counting when this page opens.">
      {timestamps.length > 1 ? (
        <>
          <AreaChart timestamps={timestamps} series={series} stacked valueFmt={(v) => v.toFixed(0)} xFmt={liveHistoryXFmt} />
          <Legend items={apNodes.map((n) => ({ label: n.name, color: nodeColor(n.name) }))} />
        </>
      ) : (
        <Empty icon="activity" title="Collecting client counts" hint="No stored history for this yet — check back in a minute." />
      )}
    </Panel>
  );
}

/** Combined up/down Mbps per AP, derived from live per-client usage joined to its associated node. */
function Throughput({ nodes, history }: { nodes: Node[]; history: ReturnType<typeof useLiveHistory> }) {
  const apNodes = nodes.filter((n) => n.radios.length > 0);
  const timestamps = history.map((p) => p.ts);
  const series = apNodes.map((n) => ({ label: n.name, color: nodeColor(n.name), values: seriesFor(history, n.name) }));
  return (
    <Panel title="Throughput per AP" subtitle="Combined up + down Mbps per access point. Starts counting when this page opens.">
      {timestamps.length > 1 ? (
        <>
          <AreaChart timestamps={timestamps} series={series} valueFmt={(v) => v.toFixed(0)} xFmt={liveHistoryXFmt} />
          <Legend items={apNodes.map((n) => ({ label: n.name, color: nodeColor(n.name) }))} />
        </>
      ) : (
        <Empty icon="activity" title="Collecting throughput" hint="No stored history for this yet — check back in a minute." />
      )}
    </Panel>
  );
}

function RadiosPanel({ nodes }: { nodes: Node[] }) {
  const rows = nodes.flatMap((n) => n.radios.map((r) => ({ node: n, radio: r })));
  return (
    <Panel title="Radios" subtitle="Per-radio RF state from each node.">
      <Table
        columns={[
          { title: "Node" }, { title: "Band", width: 80 }, { title: "Channel", width: 140 },
          { title: "Noise", width: 84, align: "right" }, { title: "Clients", width: 70, align: "right" },
        ]}
        rows={rows.map(({ node, radio }) => [
          <div><NodeTag node={node.name} /> {radio.disabled && <span class="muted small">disabled</span>}</div>,
          `${radio.band} GHz`,
          <span class="mono">{radio.channel != null ? `ch ${radio.channel}${radio.htmode ? ` · ${radio.htmode}` : ""}` : "auto"}</span>,
          <span class="mono">{dbm(radio.noise_dbm)}</span>,
          <span class="mono">{radio.clients}</span>,
        ])}
        empty="No radios reporting"
      />
    </Panel>
  );
}

function qualityColor(q: Link["quality"]): string {
  return q === "strong" ? "var(--ok)" : q === "fair" ? "var(--yellow)" : "var(--danger)";
}

function MeshPanel({ nodes }: { nodes: Node[] }) {
  const links = nodes.flatMap((n) => n.links.filter((l) => l.kind === "mesh").map((l) => ({ node: n, link: l })));
  return (
    <Panel title="Mesh backhaul" subtitle="Wireless links between nodes.">
      {links.length === 0 ? (
        <Empty icon="network" title="No mesh links reporting" />
      ) : (
        <div class="mesh-links">
          {links.map(({ node, link }) => (
            <div class="mesh-link" key={`${node.name}-${link.peer}`}>
              <div class="mesh-link-head">
                <div class="mesh-link-pair">
                  <NodeTag node={node.name} /><span class="to">to</span><NodeTag node={link.peer} />
                </div>
                <Chip label={link.quality} color={qualityColor(link.quality)} />
              </div>
              <div class="mesh-link-stats">
                {link.signal_dbm != null && <span class="mono">{dbm(link.signal_dbm)}</span>}
                {link.rx_mbps != null && <span class="mono">↓ {link.rx_mbps.toFixed(0)} Mb</span>}
                {link.tx_mbps != null && <span class="mono">↑ {link.tx_mbps.toFixed(0)} Mb</span>}
              </div>
            </div>
          ))}
        </div>
      )}
    </Panel>
  );
}
