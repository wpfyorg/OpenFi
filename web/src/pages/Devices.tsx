// Devices: filterable client list with a top-talkers-over-time chart, sortable table, and a detail
// drawer wired to the kick / reserve / whitelist actions.
import { useMemo, useState } from "preact/hooks";
import { AreaChart } from "../components/charts/AreaChart";
import { ClientDrawer } from "../components/ClientDrawer";
import { Chip } from "../components/Chip";
import { Empty } from "../components/States";
import { Legend } from "../components/Legend";
import { liveHistoryXFmt, seriesFor, useLiveHistory } from "../components/liveHistory";
import { NodeTag } from "../components/NodeTag";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { ShareBar } from "../components/ShareBar";
import { Segmented } from "../components/Segmented";
import { Table, type Column } from "../components/Table";
import { Icon } from "../components/icons";
import { useSnap } from "../context";
import { bitrate, bytes, dbm, signalTone } from "../format";
import { qosTiers } from "../components/contractExt";
import { deviceColor } from "../tokens";
import type { Client, Snapshot } from "../types";

const SECTION = "#E0609A";
type ConnFilter = "All" | "WiFi" | "Wired";
type SortKey = "name" | "conn" | "down" | "up" | "month";

export default function Devices() {
  const { snap, connected } = useSnap();
  const [query, setQuery] = useState("");
  const [conn, setConn] = useState<ConnFilter>("All");
  const [activeOnly, setActiveOnly] = useState(false);
  const [whitelistedOnly, setWhitelistedOnly] = useState(false);
  const [sortKey, setSortKey] = useState<SortKey>("month");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [selectedMac, setSelectedMac] = useState<string | null>(null);
  const [whitelistOverride, setWhitelistOverride] = useState<Record<string, boolean>>({});

  const talkHistory = useLiveHistory(snap, (s) => {
    const out: Record<string, number> = {};
    for (const cu of s.usage?.clients ?? []) out[cu.mac] = cu.rx_bps + cu.tx_bps;
    return out;
  });

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="monitor-smartphone" title="Devices" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="monitor-smartphone" title="Waiting for the first poll…" /></Panel>
      ) : (
        <>
          <TopTalkersOverTime usage={snap.usage} history={talkHistory} />
          <Panel
            title="All devices"
            subtitle="Every client OpenFi has seen. Click a row for details and actions."
          >
            <Filters query={query} onQuery={setQuery} conn={conn} onConn={setConn}
              activeOnly={activeOnly} onActiveOnly={setActiveOnly}
              whitelistedOnly={whitelistedOnly} onWhitelistedOnly={setWhitelistedOnly} />
            <DevicesTable
              snap={snap} query={query} conn={conn} activeOnly={activeOnly} whitelistedOnly={whitelistedOnly}
              sortKey={sortKey} sortDir={sortDir}
              onSort={(k) => { if (k === sortKey) setSortDir((d) => (d === "asc" ? "desc" : "asc")); else { setSortKey(k); setSortDir("desc"); } }}
              whitelistOverride={whitelistOverride}
              onSelect={setSelectedMac}
            />
          </Panel>
        </>
      )}
      {snap && selectedMac && (() => {
        const client = snap.clients.find((c) => c.mac === selectedMac);
        if (!client) return null;
        const usage = snap.usage?.clients.find((c) => c.mac === selectedMac) ?? null;
        const whitelisted = whitelistOverride[selectedMac] ?? client.whitelisted;
        return (
          <ClientDrawer
            client={client} usage={usage} whitelisted={whitelisted}
            onWhitelistChange={(on) => setWhitelistOverride((m) => ({ ...m, [selectedMac]: on }))}
            onClose={() => setSelectedMac(null)}
          />
        );
      })()}
    </div>
  );
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  const online = snap.clients.filter((c) => c.online);
  const wifi = online.filter((c) => !c.wired).length;
  return `${snap.clients.length} known devices, ${online.length} online now (${wifi} on WiFi, ${online.length - wifi} wired).`;
}

function Filters({
  query, onQuery, conn, onConn, activeOnly, onActiveOnly, whitelistedOnly, onWhitelistedOnly,
}: {
  query: string; onQuery: (v: string) => void; conn: ConnFilter; onConn: (v: ConnFilter) => void;
  activeOnly: boolean; onActiveOnly: (v: boolean) => void; whitelistedOnly: boolean; onWhitelistedOnly: (v: boolean) => void;
}) {
  return (
    <div class="filters-bar">
      <label class="search">
        <Icon name="search" size={14} color="var(--text-3)" />
        <input
          value={query} onInput={(e) => onQuery((e.target as HTMLInputElement).value)}
          placeholder="Search name, IP or MAC" aria-label="Search devices"
        />
      </label>
      <Segmented options={["All", "WiFi", "Wired"] as ConnFilter[]} value={conn} onChange={onConn} />
      <button type="button" class={activeOnly ? "toggle-btn on" : "toggle-btn"} aria-pressed={activeOnly} onClick={() => onActiveOnly(!activeOnly)}>
        Active now
      </button>
      <button type="button" class={whitelistedOnly ? "toggle-btn on" : "toggle-btn"} aria-pressed={whitelistedOnly} onClick={() => onWhitelistedOnly(!whitelistedOnly)}>
        <Icon name="shield-check" size={13} /> Whitelisted
      </button>
    </div>
  );
}

/** Per-device rx+tx rate over time, top 4 devices by month-to-date bytes plus an "Others" line. */
function TopTalkersOverTime({ usage, history }: { usage: Snapshot["usage"]; history: ReturnType<typeof useLiveHistory> }) {
  const clients = usage?.clients ?? [];
  const topMacs = [...clients].sort((a, b) => (b.rx_bytes + b.tx_bytes) - (a.rx_bytes + a.tx_bytes)).slice(0, 4).map((c) => c.mac);
  const nameOf = (mac: string) => clients.find((c) => c.mac === mac)?.hostname ?? mac;
  const timestamps = history.map((p) => p.ts);
  // A single consistent unit (Mbps) for the whole chart — mixing bps/Kbps/Mbps per-tick (bitrate())
  // makes uPlot's auto-picked axis ticks jump units unevenly and read as nonsense.
  const toMbps = (values: number[]) => values.map((v) => v / 1e6);
  const topSeries = topMacs.map((mac, i) => ({ label: nameOf(mac), color: deviceColor(i), values: toMbps(seriesFor(history, mac)) }));
  const othersValues = toMbps(history.map((p) =>
    Object.entries(p.values).filter(([k]) => !topMacs.includes(k)).reduce((a, [, v]) => a + v, 0)));
  const series = [...topSeries, { label: "Others", color: deviceColor(4), values: othersValues }];

  return (
    <Panel title="Top talkers over time" subtitle="Live rate in Mbps by device. Starts counting when this page opens.">
      {timestamps.length > 1 && topMacs.length > 0 ? (
        <>
          <AreaChart timestamps={timestamps} series={series} valueFmt={(v) => v.toFixed(2)} xFmt={liveHistoryXFmt} />
          <Legend items={series.map((s) => ({ label: s.label, color: s.color }))} />
        </>
      ) : (
        <Empty icon="activity" title="Collecting live rates" hint="No stored history for this yet — check back in a minute." />
      )}
    </Panel>
  );
}

function DevicesTable({
  snap, query, conn, activeOnly, whitelistedOnly, sortKey, sortDir, onSort, whitelistOverride, onSelect,
}: {
  snap: Snapshot; query: string; conn: ConnFilter; activeOnly: boolean; whitelistedOnly: boolean;
  sortKey: SortKey; sortDir: "asc" | "desc"; onSort: (k: SortKey) => void;
  whitelistOverride: Record<string, boolean>; onSelect: (mac: string) => void;
}) {
  const usageByMac = useMemo(() => new Map((snap.usage?.clients ?? []).map((c) => [c.mac, c])), [snap.usage]);
  const tierByMac = useMemo(() => new Map((snap.qos?.clients ?? []).map((c) => [c.mac, c])), [snap.qos]);
  const tierLabel = useMemo(() => new Map((snap.qos ? qosTiers(snap.qos) : []).map((t) => [t.id, t.label])), [snap.qos]);
  // Same ranking as the top-talkers chart (month bytes), so a share bar matches its chart line.
  const colorIndex = useMemo(() => {
    const ranked = [...(snap.usage?.clients ?? [])].sort((a, b) => (b.rx_bytes + b.tx_bytes) - (a.rx_bytes + a.tx_bytes));
    return new Map(ranked.map((c, i) => [c.mac, Math.min(i, 4)]));
  }, [snap.usage]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return snap.clients.filter((c) => {
      if (conn === "WiFi" && c.wired) return false;
      if (conn === "Wired" && !c.wired) return false;
      if (activeOnly && !c.online) return false;
      const wl = whitelistOverride[c.mac] ?? c.whitelisted;
      if (whitelistedOnly && !wl) return false;
      if (q && !(c.hostname?.toLowerCase().includes(q) || c.ip?.toLowerCase().includes(q) || c.mac.toLowerCase().includes(q))) return false;
      return true;
    });
  }, [snap.clients, query, conn, activeOnly, whitelistedOnly, whitelistOverride]);

  const maxTotal = Math.max(1, ...[...usageByMac.values()].map((u) => u.rx_bytes + u.tx_bytes));

  const sorted = useMemo(() => {
    const val = (c: Client): number | string => {
      const u = usageByMac.get(c.mac);
      switch (sortKey) {
        case "name": return (c.hostname ?? c.mac).toLowerCase();
        case "conn": return c.wired ? "" : (c.node ?? "");
        case "down": return u?.rx_bps ?? -1;
        case "up": return u?.tx_bps ?? -1;
        case "month": return u ? u.rx_bytes + u.tx_bytes : -1;
      }
    };
    const dir = sortDir === "asc" ? 1 : -1;
    return [...filtered].sort((a, b) => {
      const va = val(a), vb = val(b);
      if (typeof va === "string" || typeof vb === "string") return String(va).localeCompare(String(vb)) * dir;
      return (va - vb) * dir;
    });
  }, [filtered, sortKey, sortDir, usageByMac]);

  const columns: Column[] = [
    { title: "Device", onSort: () => onSort("name"), sortDir: sortKey === "name" ? sortDir : undefined },
    { title: "Connection", width: 220, onSort: () => onSort("conn"), sortDir: sortKey === "conn" ? sortDir : undefined },
    { title: "Down now", width: 100, align: "right", onSort: () => onSort("down"), sortDir: sortKey === "down" ? sortDir : undefined },
    { title: "Up now", width: 100, align: "right", onSort: () => onSort("up"), sortDir: sortKey === "up" ? sortDir : undefined },
    { title: "Month", width: 90, align: "right", onSort: () => onSort("month"), sortDir: sortKey === "month" ? sortDir : undefined },
    { title: "QoS tier", width: 130 },
    { title: "Share", width: 130, align: "right" },
  ];

  return (
    <div class="table-scroll">
      <Table
        columns={columns}
        rowKey={(i) => sorted[i].mac}
        onRowClick={(i) => onSelect(sorted[i].mac)}
        empty="No devices match these filters"
        rows={sorted.map((c) => {
          const u = usageByMac.get(c.mac);
          const q = tierByMac.get(c.mac);
          const wl = whitelistOverride[c.mac] ?? c.whitelisted;
          const total = u ? u.rx_bytes + u.tx_bytes : 0;
          return [
            <div>
              <b>{c.hostname ?? c.mac}</b>
              <div class="mono muted small">{c.ip ?? c.mac}</div>
            </div>,
            <ConnectionCell client={c} />,
            <span class="mono">{u ? bitrate(u.rx_bps) : "–"}</span>,
            <span class="mono">{u ? bitrate(u.tx_bps) : "–"}</span>,
            <span class="mono">{u ? bytes(total) : "–"}</span>,
            q ? <Chip label={wl ? "Whitelisted" : tierLabel.get(q.tier) ?? q.tier} color={wl ? "var(--ok)" : "var(--text-3)"} /> : <span class="muted small">n/a</span>,
            <div class="share-cell">
              <ShareBar frac={total / maxTotal} color={deviceColor(colorIndex.get(c.mac) ?? 4)} width={56} />
              <span class="mono small">{u ? `${((total / maxTotal) * 100).toFixed(0)}%` : "–"}</span>
            </div>,
          ];
        })}
      />
    </div>
  );
}

function ConnectionCell({ client }: { client: Client }) {
  if (client.wired) return <span class="mono">Wired{client.node ? <span class="muted"> · {client.node}</span> : ""}</span>;
  if (!client.node) return <span class="muted">Offline</span>;
  const tone = signalTone(client.signal_dbm);
  const toneColor = tone === "strong" ? "var(--ok)" : tone === "fair" ? "var(--yellow)" : "var(--danger)";
  return (
    <span class="mesh-link-pair">
      <NodeTag node={client.node} />
      {client.band && <span class="muted small">{client.band} GHz</span>}
      <span class="dot" style={{ background: toneColor }} />
      <span class="mono small muted">{dbm(client.signal_dbm)}</span>
    </span>
  );
}
