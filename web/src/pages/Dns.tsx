// DNS filter: AdGuard Home stats, an hourly query-volume chart, top blocked domains, and a link out
// to the AdGuard UI itself (served from the gateway node, port 3000).
import { AreaChart } from "../components/charts/AreaChart";
import { Empty } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { ShareBar } from "../components/ShareBar";
import { Stat, StatBand } from "../components/StatBand";
import { Table } from "../components/Table";
import { Icon } from "../components/icons";
import { useSnap } from "../context";
import { pct } from "../format";
import type { Dns, Snapshot } from "../types";

const SECTION = "#EF5F5F";

export default function Dns() {
  const { snap, connected } = useSnap();
  const gatewayNode = snap?.nodes.find((n) => n.role === "gateway");
  const adguardUrl = gatewayNode ? `http://${gatewayNode.host}:3000` : null;

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader
        icon="shield-check" title="DNS filter" subtitle={statusLine(snap, connected)}
        right={adguardUrl && (
          <a class="btn" href={adguardUrl} target="_blank" rel="noopener noreferrer">
            <Icon name="external-link" size={13} /> Open AdGuard Home
          </a>
        )}
      />
      {!snap ? (
        <Panel><Empty icon="shield-check" title="Waiting for the first poll…" /></Panel>
      ) : !snap.dns ? (
        <Panel><Empty icon="shield-check" title="No DNS data yet" hint="AdGuard Home may not be reachable from the controller." /></Panel>
      ) : (
        <>
          <DnsStats dns={snap.dns} />
          <Panel title="Queries per hour" subtitle="Last 24 hours.">
            {snap.dns.hourly_queries.length > 1 ? (
              <HourlyChart values={snap.dns.hourly_queries} />
            ) : (
              <Empty icon="activity" title="No hourly history yet" />
            )}
          </Panel>
          <TopBlocked dns={snap.dns} />
        </>
      )}
    </div>
  );
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  if (!snap.dns) return "AdGuard Home isn't reporting.";
  const blockedPct = snap.dns.queries > 0 ? (snap.dns.blocked / snap.dns.queries) * 100 : 0;
  return `${snap.dns.queries.toLocaleString()} queries, ${blockedPct.toFixed(1)}% blocked, ${snap.dns.avg_ms.toFixed(0)} ms average.`;
}

function DnsStats({ dns }: { dns: Dns }) {
  const blockedPct = dns.queries > 0 ? (dns.blocked / dns.queries) * 100 : 0;
  return (
    <StatBand>
      <Stat icon="shield-check" color="var(--section)" label="Queries" value={dns.queries.toLocaleString()} sub="Last 24 hours" />
      <Stat icon="ban" color="var(--danger)" label="Blocked" value={dns.blocked.toLocaleString()} sub={pct(blockedPct, 1)} />
      <Stat icon="gauge" color="var(--text-2)" label="Average latency" value={`${dns.avg_ms.toFixed(0)} ms`} sub="Resolver response time" />
      <Stat icon="activity" color="var(--ok)" label="Top blocked domain" value={dns.top_blocked[0]?.domain ?? "None"}
        sub={dns.top_blocked[0] ? `${dns.top_blocked[0].count.toLocaleString()} hits` : "Nothing blocked yet"} />
    </StatBand>
  );
}

function HourlyChart({ values }: { values: number[] }) {
  const now = Math.floor(Date.now() / 1000);
  const timestamps = values.map((_, i) => now - (values.length - 1 - i) * 3600);
  return (
    <AreaChart
      timestamps={timestamps}
      series={[{ label: "Queries", color: "var(--danger)", values }]}
      valueFmt={(v) => v.toFixed(0)}
      xFmt={(ts) => new Date(ts * 1000).toLocaleTimeString([], { hour: "2-digit" })}
    />
  );
}

function TopBlocked({ dns }: { dns: Dns }) {
  const max = Math.max(1, ...dns.top_blocked.map((b) => b.count));
  return (
    <Panel title="Top blocked domains" subtitle="Most-blocked hostnames in the current window.">
      <Table
        columns={[{ title: "Domain" }, { title: "Hits", width: 200, align: "right" }]}
        empty="Nothing blocked yet"
        rows={dns.top_blocked.map((b) => [
          <span class="mono">{b.domain}</span>,
          <div class="share-cell">
            <ShareBar frac={b.count / max} color="var(--danger)" width={100} />
            <span class="mono small">{b.count.toLocaleString()}</span>
          </div>,
        ])}
      />
    </Panel>
  );
}
