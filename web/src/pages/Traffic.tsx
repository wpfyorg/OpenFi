// Traffic: WAN rate over a switchable 24h/7d window, the live application mix, and top talkers by
// current rate (as opposed to Devices/Usage, which rank by month-to-date bytes).
import { useEffect, useState } from "preact/hooks";
import { ApiError, getHistory } from "../api";
import { AreaChart } from "../components/charts/AreaChart";
import { Empty, ErrorState, Loading } from "../components/States";
import { wanWindow } from "../components/contractExt";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Segmented } from "../components/Segmented";
import { ShareBar } from "../components/ShareBar";
import { Table } from "../components/Table";
import { useSnap } from "../context";
import { bitrate, gb, mbps } from "../format";
import { DOWN_COLOR, UP_COLOR, deviceColor } from "../tokens";
import type { AppUsage, ClientUsage, RatePoint, Snapshot } from "../types";

const SECTION = "#2AA7E0";
type Range = "24h" | "7d";

export default function Traffic() {
  const { snap, connected } = useSnap();
  const [range, setRange] = useState<Range>("24h");
  const [history, setHistory] = useState<RatePoint[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setHistory(null);
    setError(null);
    getHistory(range).then(
      (rows) => { if (!cancelled) setHistory(rows); },
      (e) => { if (!cancelled) setError(e instanceof ApiError ? e.message : "Couldn't load WAN history"); },
    );
    return () => { cancelled = true; };
  }, [range]);

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="arrow-left-right" title="Traffic" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="activity" title="Waiting for the first poll…" /></Panel>
      ) : (
        <>
          <WanPanel snap={snap} range={range} onRange={setRange} history={history} error={error} />
          <div class="row row-bottom">
            <AppMix apps={snap.usage?.apps ?? null} />
            <TopTalkersNow usage={snap.usage?.clients ?? null} />
          </div>
        </>
      )}
    </div>
  );
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  return `${mbps(snap.wan.rx_bps)} Mbps down, ${mbps(snap.wan.tx_bps)} Mbps up right now.`;
}

function WanPanel({
  snap, range, onRange, history, error,
}: { snap: Snapshot; range: Range; onRange: (r: Range) => void; history: RatePoint[] | null; error: string | null }) {
  const { wan } = snap;
  const win = wanWindow(wan);
  const rows = history ?? [];
  const timestamps = rows.map((p) => p.ts);
  const dl = rows.map((p) => p.rx_bps / 1e6);
  const ul = rows.map((p) => p.tx_bps / 1e6);

  return (
    <Panel
      title="WAN throughput"
      subtitle={wan.ip ? `${wan.ip} via ${wan.gateway ?? "unknown gateway"}` : "No WAN address"}
      right={<Segmented options={["24h", "7d"] as Range[]} value={range} onChange={onRange} />}
    >
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
          <span class="label">Last 24 hours</span>
          <span class="mono muted">{win.rx != null ? `${gb(win.rx)} GB down` : "n/a"}</span>
          <span class="mono muted">{win.tx != null ? `${gb(win.tx)} GB up` : "n/a"}</span>
        </div>
        <div class="readout-cell readout-since">
          <span class="label">Since boot</span>
          <span class="mono muted">{gb(wan.rx_bytes)} GB down</span>
          <span class="mono muted">{gb(wan.tx_bytes)} GB up</span>
        </div>
      </div>
      {error ? (
        <ErrorState message={error} />
      ) : !history ? (
        <Loading label={`Loading ${range} history…`} />
      ) : timestamps.length > 1 ? (
        <AreaChart
          timestamps={timestamps}
          series={[{ label: "Download", color: DOWN_COLOR, values: dl }]}
          mirror={[{ label: "Upload", color: UP_COLOR, values: ul }]}
          mirrorFrac={0.28}
          valueFmt={(v) => v.toFixed(0)}
        />
      ) : (
        <Empty icon="activity" title="No history for this window yet" />
      )}
      <div class="legend">
        <span class="legend-item"><i style={{ background: DOWN_COLOR }} />Download</span>
        <span class="legend-item"><i style={{ background: UP_COLOR }} />Upload, drawn below the axis</span>
      </div>
    </Panel>
  );
}

function AppMix({ apps }: { apps: AppUsage[] | null }) {
  if (!apps || apps.length === 0) {
    return <Panel title="Application mix" subtitle="Share of month-to-date bytes by app."><Empty icon="chart-column" title="No application data yet" /></Panel>;
  }
  const rows = [...apps].sort((a, b) => (b.rx_bytes + b.tx_bytes) - (a.rx_bytes + a.tx_bytes)).slice(0, 8);
  const max = Math.max(1, ...rows.map((a) => a.rx_bytes + a.tx_bytes));
  return (
    <Panel title="Application mix" subtitle="Share of month-to-date bytes by app.">
      <div class="share-list">
        {rows.map((a, i) => (
          <div class="share-list-row" key={a.name}>
            <span class="name">{a.name}</span>
            <ShareBar frac={(a.rx_bytes + a.tx_bytes) / max} color={deviceColor(i)} width={80} />
            <span class="mono small" style={{ width: 56, textAlign: "right" }}>{gb(a.rx_bytes + a.tx_bytes)} GB</span>
          </div>
        ))}
      </div>
    </Panel>
  );
}

function TopTalkersNow({ usage }: { usage: ClientUsage[] | null }) {
  if (!usage || usage.length === 0) {
    return <Panel title="Top talkers now" subtitle="Ranked by current rate, not month-to-date total."><Empty icon="chart-column" title="No live rate data yet" /></Panel>;
  }
  const rows = [...usage].filter((c) => c.rx_bps + c.tx_bps > 0).sort((a, b) => (b.rx_bps + b.tx_bps) - (a.rx_bps + a.tx_bps)).slice(0, 8);
  return (
    <Panel title="Top talkers now" subtitle="Ranked by current rate, not month-to-date total.">
      <Table
        columns={[{ title: "Device" }, { title: "Now", width: 130, align: "right" }]}
        empty="Nothing is transferring data right now"
        rows={rows.map((c) => [
          <div><b>{c.hostname ?? c.mac}</b><div class="mono muted small">{c.ip ?? c.mac}</div></div>,
          <div class="updown">
            <span><span class="arrow down">↓</span><span class="mono">{bitrate(c.rx_bps)}</span></span>
            <span><span class="arrow up">↑</span><span class="mono">{bitrate(c.tx_bps)}</span></span>
          </div>,
        ])}
      />
    </Panel>
  );
}
