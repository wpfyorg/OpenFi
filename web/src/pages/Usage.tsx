// Usage: monthly accounting — stat band, a daily bar chart from GET /usage/daily, a daily table,
// and per-device fair-use progress toward the next qos.tiers threshold.
import { useEffect, useState } from "preact/hooks";
import { ApiError, getUsageDaily } from "../api";
import { BarChart } from "../components/charts/BarChart";
import { qosTiers } from "../components/contractExt";
import { Empty, ErrorState, Loading } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Stat, StatBand } from "../components/StatBand";
import { Table } from "../components/Table";
import { tierProgress } from "../components/tiers";
import { useSnap } from "../context";
import { bytes, gb } from "../format";
import { DOWN_COLOR, UP_COLOR } from "../tokens";
import type { DailyUsage, Snapshot } from "../types";

const SECTION = "#E8A33C";

export default function Usage() {
  const { snap, connected } = useSnap();
  const [daily, setDaily] = useState<DailyUsage[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reloadTick, setReloadTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    getUsageDaily(30).then(
      (rows) => { if (!cancelled) setDaily(rows); },
      (e) => { if (!cancelled) setError(e instanceof ApiError ? e.message : "Couldn't load daily usage"); },
    );
    return () => { cancelled = true; };
  }, [reloadTick]);

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="chart-column" title="Usage" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="chart-column" title="Waiting for the first poll…" /></Panel>
      ) : (
        <>
          <UsageStats snap={snap} daily={daily} />
          <Panel title="Usage per day" subtitle={daily ? `Last ${daily.length} days.` : "Last 30 days."}>
            {error ? (
              <ErrorState message={error} onRetry={() => setReloadTick((t) => t + 1)} />
            ) : !daily ? (
              <Loading label="Loading daily usage…" />
            ) : daily.length === 0 ? (
              <Empty icon="chart-column" title="No daily usage recorded yet" />
            ) : (
              <>
                <BarChart
                  groups={daily.map((d) => ({
                    label: dayLabel(d.day),
                    segments: [{ value: d.rx_bytes, color: DOWN_COLOR }, { value: d.tx_bytes, color: UP_COLOR }],
                  }))}
                  valueFmt={(v) => `${gb(v)} GB`}
                />
                <div class="legend">
                  <span class="legend-item"><i style={{ background: DOWN_COLOR }} />Download</span>
                  <span class="legend-item"><i style={{ background: UP_COLOR }} />Upload</span>
                </div>
              </>
            )}
          </Panel>
          {daily && daily.length > 0 && <DailyTable daily={daily} />}
          <MonthlyFairUse snap={snap} />
        </>
      )}
    </div>
  );
}

function dayLabel(day: string): string {
  const d = new Date(`${day}T00:00:00`);
  if (Number.isNaN(d.getTime())) return day;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  const usage = snap.usage;
  if (!usage) return "No monthly usage data yet.";
  const total = usage.total_rx_bytes + usage.total_tx_bytes;
  return `${usage.client_count} devices have used ${gb(total)} GB this month.`;
}

function UsageStats({ snap, daily }: { snap: Snapshot; daily: DailyUsage[] | null }) {
  const usage = snap.usage;
  const qos = snap.qos;
  const rx = usage?.total_rx_bytes ?? 0;
  const tx = usage?.total_tx_bytes ?? 0;
  const tiers = qos ? qosTiers(qos) : [];
  const nearLimit = qos ? usage?.clients
    .map((c) => ({ c, p: tierProgress(c.rx_bytes + c.tx_bytes, tiers) }))
    .filter((x) => x.p.frac != null && x.p.frac >= 0.9) ?? [] : [];
  const worst = [...nearLimit].sort((a, b) => (b.p.frac ?? 0) - (a.p.frac ?? 0))[0];

  return (
    <StatBand>
      <Stat icon="arrow-down" color="var(--down)" label="Downloaded" value={usage ? `${gb(rx)} GB` : "n/a"}
        sub={daily ? `${daily.length} days in window` : "This month"} />
      <Stat icon="arrow-up" color="var(--up)" label="Uploaded" value={usage ? `${gb(tx)} GB` : "n/a"}
        sub="This month" />
      <Stat icon="sigma" color="var(--text-2)" label="Total" value={usage ? `${gb(rx + tx)} GB` : "n/a"}
        sub={usage ? `${usage.client_count} devices` : "No usage data yet"} />
      <Stat icon="monitor-smartphone" color="#E0609A" label="Active devices" value={usage ? usage.client_count : "n/a"}
        sub="Distinct devices with usage this month" />
      <Stat icon="gauge" color="var(--warn)" label="Near fair-use limit" value={qos ? nearLimit.length : "n/a"}
        sub={worst ? `${worst.c.hostname ?? worst.c.mac} at ${((worst.p.frac ?? 0) * 100).toFixed(0)}% of its tier` : qos ? "None over 90% yet" : "No QoS data yet"} />
    </StatBand>
  );
}

function DailyTable({ daily }: { daily: DailyUsage[] }) {
  const rows = [...daily].reverse();
  return (
    <Panel title="Daily breakdown" subtitle="Newest first.">
      <div class="table-scroll">
        <Table
          columns={[
            { title: "Day" }, { title: "Down", width: 100, align: "right" }, { title: "Up", width: 100, align: "right" },
            { title: "Total", width: 100, align: "right" }, { title: "Devices", width: 90, align: "right" },
          ]}
          rows={rows.map((d) => [
            d.day,
            <span class="mono">{bytes(d.rx_bytes)}</span>,
            <span class="mono">{bytes(d.tx_bytes)}</span>,
            <span class="mono">{bytes(d.rx_bytes + d.tx_bytes)}</span>,
            <span class="mono">{d.devices}</span>,
          ])}
        />
      </div>
    </Panel>
  );
}

function MonthlyFairUse({ snap }: { snap: Snapshot }) {
  const usage = snap.usage;
  const qos = snap.qos;
  if (!usage || usage.clients.length === 0) {
    return <Panel title="Monthly fair use" subtitle="Per device, resets on the billing day."><Empty icon="gauge" title="No usage data yet" /></Panel>;
  }
  const tiers = qos ? qosTiers(qos) : [];
  const rows = [...usage.clients].sort((a, b) => (b.rx_bytes + b.tx_bytes) - (a.rx_bytes + a.tx_bytes));

  return (
    <Panel title="Monthly fair use" subtitle="Per device · progress toward the next tier, resets on the billing day.">
      {rows.map((c) => {
        const total = c.rx_bytes + c.tx_bytes;
        const p = tierProgress(total, tiers);
        const over = p.frac != null && p.frac >= 0.9;
        return (
          <div class="fup-row" key={c.mac}>
            <div class="fup-head">
              <span><b>{c.hostname ?? c.mac}</b>{p.tier && <span class="muted"> · {p.tier.label}</span>}</span>
              <span class="mono">
                {gb(total)} GB{p.tier?.max_gb != null ? ` / ${p.tier.max_gb} GB` : p.next ? ` / ${p.next.min_gb} GB` : ""}
              </span>
            </div>
            {p.frac != null ? (
              <div class="fup-bar"><span style={{ width: `${p.frac * 100}%`, background: over ? "var(--warn)" : "var(--section)" }} /></div>
            ) : (
              <span class="muted small">No tier ladder yet — showing total usage only.</span>
            )}
          </div>
        );
      })}
    </Panel>
  );
}
