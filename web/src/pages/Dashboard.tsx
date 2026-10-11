import { useEffect, useState } from "preact/hooks";
import { getHistory } from "../api";
import { AreaChart } from "../components/charts/AreaChart";
import { Sparkline } from "../components/charts/Sparkline";
import { Chip } from "../components/Chip";
import { Empty } from "../components/States";
import { liveHistoryXFmt, seriesFor, useLiveHistory } from "../components/liveHistory";
import { NodeTag } from "../components/NodeTag";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { ShareBar } from "../components/ShareBar";
import { Stat, StatBand } from "../components/StatBand";
import { Table } from "../components/Table";
import { useSnap } from "../context";
import { bitrate, bytes, gb, mbps } from "../format";
import { DOWN_COLOR, UP_COLOR, deviceColor } from "../tokens";
import type { ClientUsage, Event, Link, Node, RatePoint } from "../types";

const SECTION = "#4E8BFF";

export default function Dashboard() {
  const { snap, connected } = useSnap();
  const [week, setWeek] = useState<RatePoint[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    getHistory("7d").then((rows) => { if (!cancelled) setWeek(rows); }, () => {});
    return () => { cancelled = true; };
  }, []);

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="gauge" title="Dashboard" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="activity" title="Waiting for the first poll…" /></Panel>
      ) : (
        <>
          <TopStats snap={snap} />
          <div class="row row-top">
            <Internet snap={snap} week={week} />
            <TopTalkers usage={snap.usage?.clients ?? null} />
          </div>
          <div class="row row-bottom">
            <NodesPanel nodes={snap.nodes} events={snap.events} />
            <QosPanel qos={snap.qos} />
            <DnsPanel dns={snap.dns} />
          </div>
        </>
      )}
    </div>
  );
}

/** Builds the page header's live one-liner from health + client + throughput data. */
function statusLine(snap: ReturnType<typeof useSnap>["snap"], connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  const total = snap.nodes.length;
  const online = snap.nodes.filter((n) => n.online).length;
  const clientsOnline = snap.clients.filter((c) => c.online).length;
  const downMbps = Math.round(snap.wan.rx_bps / 1e6);
  const health = online === total && snap.health.status === "ok"
    ? `All ${total} node${total === 1 ? "" : "s"} healthy.`
    : `${online}/${total} nodes online${snap.health.reasons[0] ? ` – ${snap.health.reasons[0]}` : "."}`;
  return `${health} ${clientsOnline} devices online, ${downMbps} Mbps down right now.`;
}

/** Stat band: devices online, WiFi clients, health, and this month's total. */
function TopStats({ snap }: { snap: NonNullable<ReturnType<typeof useSnap>["snap"]> }) {
  const online = snap.clients.filter((c) => c.online);
  const wired = online.filter((c) => c.wired).length;
  const wifi = online.filter((c) => !c.wired);
  const on5 = wifi.filter((c) => c.band === "5").length;
  const apCount = snap.nodes.filter((n) => n.radios.length > 0).length;
  const usage = snap.usage;
  const totalRx = usage?.total_rx_bytes ?? 0;
  const totalTx = usage?.total_tx_bytes ?? 0;
  const healthColor = snap.health.status === "ok" ? "var(--ok)" : snap.health.status === "degraded" ? "var(--warn)" : "var(--danger)";
  const healthLabel = snap.health.status === "ok" ? "Healthy" : snap.health.status === "degraded" ? "Degraded" : "Down";

  return (
    <StatBand>
      <Stat icon="monitor-smartphone" color="#E0609A" label="Devices online" value={online.length}
        sub={`${wifi.length} on WiFi, ${wired} wired`} />
      <Stat icon="wifi" color="#9476F0" label="WiFi clients" value={wifi.length}
        sub={`${on5} on 5 GHz across ${apCount} access point${apCount === 1 ? "" : "s"}`} />
      <Stat icon="heart" color={healthColor} label="Health"
        value={<span class="stat-dot-value"><span class="dot" style={{ background: healthColor }} />{healthLabel}</span>}
        sub={snap.health.reasons[0] ?? `All ${snap.nodes.length} nodes up.`} />
      <Stat icon="calendar" color="#E8A33C" label="This month"
        value={usage ? `${gb(totalRx + totalTx)} GB` : "–"}
        sub={usage ? `${gb(totalRx)} GB down, ${gb(totalTx)} GB up` : "No usage data yet"} />
    </StatBand>
  );
}

/** The big live readout + 24h chart + 7-day trend strip. */
function Internet({ snap, week }: { snap: NonNullable<ReturnType<typeof useSnap>["snap"]>; week: RatePoint[] | null }) {
  const { wan } = snap;
  // A fresh server has at most one 30-min bucket; until there are two, chart the live 2 s samples.
  const live = useLiveHistory(snap, (s) => ({ rx: s.wan.rx_bps, tx: s.wan.tx_bps }));
  const useLive = wan.history.length < 2;
  const timestamps = useLive ? live.map((p) => p.ts) : wan.history.map((p) => p.ts);
  const dl = (useLive ? seriesFor(live, "rx") : wan.history.map((p) => p.rx_bps)).map((v) => v / 1e6);
  const ul = (useLive ? seriesFor(live, "tx") : wan.history.map((p) => p.tx_bps)).map((v) => v / 1e6);
  const hasWindow = wan.window_rx_bytes > 0 || wan.window_tx_bytes > 0;
  const weekVals = week ? week.map((p) => (p.rx_bps + p.tx_bps) / 2e6) : [];

  return (
    <Panel class="panel-internet" title="Internet" subtitle="Mbps, live and over the last 24 hours. Drag the chart to zoom.">
      <div class="readout">
        <div class="readout-cell">
          <span class="label">Download now</span>
          <span class="readout-value down"><span class="arrow">↓</span><span class="mono">{mbps(wan.rx_bps)}</span><small>Mbps</small></span>
        </div>
        <div class="readout-cell">
          <span class="label">Upload now</span>
          <span class="readout-value up"><span class="arrow">↑</span><span class="mono">{mbps(wan.tx_bps)}</span><small>Mbps</small></span>
        </div>
        <div class="readout-cell readout-since">
          <span class="label">{hasWindow ? "Last 24 hours" : "Since boot"}</span>
          <span class="mono muted">{gb(hasWindow ? wan.window_rx_bytes : wan.rx_bytes)} GB down</span>
          <span class="mono muted">{gb(hasWindow ? wan.window_tx_bytes : wan.tx_bytes)} GB up</span>
        </div>
      </div>
      {timestamps.length > 1 ? (
        <AreaChart
          timestamps={timestamps}
          series={[{ label: "Download", color: DOWN_COLOR, values: dl }]}
          mirror={[{ label: "Upload", color: UP_COLOR, values: ul }]}
          mirrorFrac={0.28}
          valueFmt={(v) => v.toFixed(0)}
          xFmt={useLive ? liveHistoryXFmt : undefined}
        />
      ) : (
        <Empty icon="activity" title="Collecting samples…" />
      )}
      {useLive && timestamps.length > 1 && <p class="state-hint">Live samples since this page opened. The 24-hour view fills in as history builds up.</p>}
      <div class="legend">
        <span class="legend-item"><i style={{ background: DOWN_COLOR }} />Download</span>
        <span class="legend-item"><i style={{ background: UP_COLOR }} />Upload, drawn below the axis</span>
      </div>
      <div class="overview">
        <span class="label">Last 7 days</span>
        {weekVals.length > 1 ? <Sparkline values={weekVals} color={DOWN_COLOR} width={640} height={34} /> : <Empty icon="activity" title="No 7-day history yet" />}
      </div>
    </Panel>
  );
}

/** Per-client month-to-date usage, ranked by total bytes, from nlbwmon. */
function TopTalkers({ usage }: { usage: ClientUsage[] | null }) {
  if (!usage) {
    return <Panel title="Top talkers" subtitle="Bytes in the window, by device."><Empty icon="chart-column" title="No usage data yet" /></Panel>;
  }
  const rows = [...usage].sort((a, b) => (b.rx_bytes + b.tx_bytes) - (a.rx_bytes + a.tx_bytes)).slice(0, 8);
  const max = Math.max(1, ...rows.map((c) => c.rx_bytes + c.tx_bytes));
  return (
    <Panel title="Top talkers" subtitle="Bytes in the window, by device.">
      <Table
        columns={[{ title: "Device" }, { title: "Now", width: 108, align: "right" }, { title: "Share", width: 120, align: "right" }]}
        rows={rows.map((c, i) => [
          <div>
            <b>{c.hostname ?? c.mac}</b>
            <div class="mono muted small">{c.ip ?? c.mac}{c.top_apps.length ? ` · ${c.top_apps.slice(0, 2).join(", ")}` : ""}</div>
          </div>,
          <div class="updown">
            <span><span class="arrow down">↓</span><span class="mono">{bitrate(c.rx_bps)}</span></span>
            <span><span class="arrow up">↑</span><span class="mono">{bitrate(c.tx_bps)}</span></span>
          </div>,
          <div class="share-cell">
            <ShareBar frac={(c.rx_bytes + c.tx_bytes) / max} color={deviceColor(i)} width={56} />
            <span class="mono small">{bytes(c.rx_bytes + c.tx_bytes)}</span>
          </div>,
        ])}
      />
    </Panel>
  );
}

/** Best link summary for a node's mesh row: wired uplink if present, else the strongest mesh link. */
function bestLink(links: Link[]): Link | null {
  return links.find((l) => l.kind === "wan") ?? links.find((l) => l.kind === "wired") ?? links.find((l) => l.kind === "mesh") ?? null;
}

function roleLabel(node: Node): string {
  if (node.role === "gateway") return "gateway";
  const wired = node.links.some((l) => l.kind === "wired");
  return wired ? "wired + mesh" : "mesh only";
}

function qualityColor(q: Link["quality"] | undefined): string {
  return q === "strong" ? "var(--ok)" : q === "fair" ? "var(--yellow)" : "var(--danger)";
}

/** Nodes and mesh: identity, link quality, clients, temperature, CPU/RAM, and a usteer summary. */
function NodesPanel({ nodes, events }: { nodes: Node[]; events: Event[] }) {
  const dayAgo = Date.now() / 1000 - 86400;
  const steered = events.filter((e) => (e.kind === "roam" || e.kind === "band_steer") && e.ts >= dayAgo);
  return (
    <Panel title="Nodes and mesh" subtitle="Clients, temperature and load per node.">
      <div class="node-rows">
        {nodes.map((n) => {
          const link = bestLink(n.links);
          const clients = n.radios.reduce((a, r) => a + r.clients, 0);
          const hotTemp = n.temps.length ? Math.max(...n.temps.map((t) => t.celsius)) : null;
          const hot = hotTemp != null && hotTemp >= 70;
          const cpu = Math.min(99, Math.round(n.load[0] * 100));
          const ram = n.mem_total > 0 ? Math.round(((n.mem_total - n.mem_available) / n.mem_total) * 100) : 0;
          return (
            <div class="node-row" key={n.name}>
              <div class="node-row-who">
                <NodeTag node={n.name} size={13} />
                <span class="muted small">{roleLabel(n)}</span>
                {link && (
                  <span class="node-row-link">
                    <span class="dot" style={{ background: n.online ? qualityColor(link.quality) : "var(--danger)" }} />
                    <span class="mono small">
                      {link.kind === "mesh" && link.signal_dbm != null ? `mesh ${link.signal_dbm} dBm` : link.peer}
                      {link.rx_mbps != null ? `, ${Math.round(link.rx_mbps)} Mb` : ""}
                    </span>
                  </span>
                )}
              </div>
              <span class="mono muted node-row-clients">{clients}</span>
              <span class={hot ? "node-row-temp hot" : "node-row-temp"}>
                {hotTemp != null ? `${Math.round(hotTemp)}°` : "–"}
              </span>
              <Meter label="CPU" value={cpu} color="var(--section)" />
              <Meter label="RAM" value={ram} color={ram > 55 ? "var(--warn)" : "var(--section)"} />
            </div>
          );
        })}
        {nodes.length === 0 && <Empty icon="router" title="No nodes reporting" />}
      </div>
      {steered.length > 0 && (
        <div class="callout">
          <span class="mono">⇄</span>
          <span>In the last 24 hours usteer moved {steered.length} client{steered.length === 1 ? "" : "s"} between nodes.</span>
        </div>
      )}
    </Panel>
  );
}

function Meter({ label, value, color }: { label: string; value: number; color: string }) {
  return (
    <div class="meter">
      <div class="meter-head"><span class="label">{label}</span><span class="mono small">{value}%</span></div>
      <ShareBar frac={value / 100} color={color} width={64} />
    </div>
  );
}

/** Fair use tiers, grouped from qos.clients — the contract has no fixed tier table, just per-client tier names. */
function QosPanel({ qos }: { qos: NonNullable<ReturnType<typeof useSnap>["snap"]>["qos"] }) {
  if (!qos) return <Panel title="Fair use" subtitle="Monthly tiers."><Empty icon="sliders-horizontal" title="No QoS data yet" /></Panel>;
  const byTier = new Map<string, { count: number; whitelisted: number }>();
  for (const c of qos.clients) {
    const e = byTier.get(c.tier) ?? { count: 0, whitelisted: 0 };
    e.count += 1;
    if (c.whitelisted) e.whitelisted += 1;
    byTier.set(c.tier, e);
  }
  const shaperOff = qos.shaper_dl_kbit === 0 && qos.shaper_ul_kbit === 0;
  return (
    <Panel
      title="Fair use"
      subtitle="Monthly tiers, counted from qos-tiers."
      right={<Chip label={qos.enforcing_monthly ? "Enforcing" : "Counting"} color={qos.enforcing_monthly ? "var(--warn)" : "var(--ok)"} />}
    >
      <div class="tier-list">
        {[...byTier.entries()].map(([tier, e]) => (
          <div class="tier-row" key={tier}>
            <span>{tier}{e.whitelisted ? ` · ${e.whitelisted} whitelisted` : ""}</span>
            <span class="mono">{e.count}</span>
          </div>
        ))}
        {byTier.size === 0 && <Empty icon="sliders-horizontal" title="No clients counted yet" />}
      </div>
      {shaperOff && (
        <div class="callout warn">
          <span class="mono">!</span>
          <span>Shaper is off, so latency rises when the line is busy.</span>
        </div>
      )}
    </Panel>
  );
}

/** DNS filter summary: queries, blocked share, latency, a 24h sparkline, and top blocked domains. */
function DnsPanel({ dns }: { dns: NonNullable<ReturnType<typeof useSnap>["snap"]>["dns"] }) {
  if (!dns) return <Panel title="DNS filter" subtitle="AdGuard Home"><Empty icon="shield-check" title="No DNS data yet" /></Panel>;
  const blockedPct = dns.queries > 0 ? (dns.blocked / dns.queries) * 100 : 0;
  return (
    <Panel title="DNS filter" subtitle="AdGuard Home">
      <div class="dns-stats">
        <div><span class="label">Queries</span><span class="mono big">{dns.queries.toLocaleString()}</span></div>
        <div><span class="label">Blocked</span><span class="mono big" style={{ color: "var(--danger)" }}>{blockedPct.toFixed(1)}%</span></div>
        <div><span class="label">Latency</span><span class="mono big">{dns.avg_ms.toFixed(0)} ms</span></div>
      </div>
      {dns.hourly_queries.length > 1 && <Sparkline values={dns.hourly_queries} color="var(--danger)" width={280} height={40} />}
      <div class="top-blocked">
        {dns.top_blocked.slice(0, 3).map((b) => (
          <div class="top-blocked-row" key={b.domain}>
            <span class="mono small">{b.domain}</span>
            <span class="mono small">{b.count.toLocaleString()}</span>
          </div>
        ))}
        {dns.top_blocked.length === 0 && <span class="muted small">Nothing blocked yet</span>}
      </div>
    </Panel>
  );
}
