// logread viewer per node, plus diagnostics (ping/traceroute/nslookup) and the audit trail.
// GET /logs?node=&lines=, GET /audit, and diagnostics via DiagPanel (POST /diag).
import { useEffect, useMemo, useState } from "preact/hooks";
import { getAudit, getLogs } from "../api";
import { Chip } from "../components/Chip";
import { DiagPanel } from "../components/DiagPanel";
import { Empty, ErrorState, Loading } from "../components/States";
import { NodeTag } from "../components/NodeTag";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Segmented } from "../components/Segmented";
import { Table } from "../components/Table";
import { Toggle } from "../components/Toggle";
import { useSnap } from "../context";
import { clock } from "../format";
import type { AuditEntry, LogLevel, LogLine } from "../types";

const SECTION = "#3CC4D8";
const TABS = ["Logs", "Diagnostics", "Audit"] as const;
type Tab = (typeof TABS)[number];
const LEVELS: (LogLevel | "all")[] = ["all", "info", "warn", "err"];
const PAGE_SIZE = 200;

export default function Logs() {
  const { snap } = useSnap();
  const [tab, setTab] = useState<Tab>("Logs");

  const subtitle = tab === "Logs" ? "logread, per node" : tab === "Diagnostics" ? "ping, traceroute, nslookup" : "Every write action, with who and when";

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="scroll-text" title="Logs" subtitle={subtitle} right={<Segmented options={[...TABS]} value={tab} onChange={setTab} />} />
      {tab === "Logs" && <LogsTab nodes={snap?.nodes.map((n) => n.name) ?? []} />}
      {tab === "Diagnostics" && <DiagPanel nodes={snap?.nodes ?? []} />}
      {tab === "Audit" && <AuditTab />}
    </div>
  );
}

function LogsTab({ nodes }: { nodes: string[] }) {
  const [node, setNode] = useState("");
  const [level, setLevel] = useState<LogLevel | "all">("all");
  const [text, setText] = useState("");
  const [autoRefresh, setAutoRefresh] = useState(false);
  const [lines, setLines] = useState(PAGE_SIZE);
  const [rows, setRows] = useState<LogLine[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  function load() {
    getLogs({ node: node || undefined, lines }).then(
      (r) => { setRows(r); setError(null); },
      (e) => setError(e instanceof Error ? e.message : "Failed to load logs"),
    );
  }
  useEffect(load, [node, lines]);
  useEffect(() => {
    if (!autoRefresh) return;
    const id = setInterval(load, 5000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRefresh, node, lines]);

  const filtered = useMemo(() => {
    if (!rows) return [];
    const needle = text.trim().toLowerCase();
    return rows.filter((r) => (level === "all" || r.level === level) && (!needle || r.msg.toLowerCase().includes(needle) || r.source.toLowerCase().includes(needle)));
  }, [rows, level, text]);

  return (
    <Panel title="Logs" subtitle={rows ? `${filtered.length} of ${rows.length} lines` : undefined}>
      <div class="logs-filters">
        <select class="text-input" value={node} onChange={(e) => setNode((e.target as HTMLSelectElement).value)} aria-label="Filter by node">
          <option value="">All nodes</option>
          {nodes.map((n) => <option value={n} key={n}>{n}</option>)}
        </select>
        <Segmented options={LEVELS} value={level} onChange={setLevel} />
        <input
          class="text-input logs-text-filter" value={text} onInput={(e) => setText((e.target as HTMLInputElement).value)}
          placeholder="Filter by text…" aria-label="Filter by text"
        />
        <label class="switch-row logs-autorefresh">
          <span class="switch-title">Auto-refresh</span>
          <Toggle checked={autoRefresh} onChange={setAutoRefresh} />
        </label>
      </div>
      {error ? (
        <ErrorState message={error} onRetry={load} />
      ) : !rows ? (
        <Loading label="Loading logs…" />
      ) : filtered.length === 0 ? (
        <Empty icon="scroll-text" title="No matching log lines" />
      ) : (
        <>
          <div class="table-scroll">
          <Table
            columns={[{ title: "Time", width: 90 }, { title: "Node", width: 110 }, { title: "Level", width: 70 }, { title: "Source", width: 140 }, { title: "Message" }]}
            rows={filtered.map((r) => [
              <span class="mono small">{clock(r.ts)}</span>,
              <NodeTag node={r.node} size={11} />,
              <Chip label={r.level} color={levelColor(r.level)} />,
              <span class="mono small muted">{r.source}</span>,
              <span class="mono small log-msg">{r.msg}</span>,
            ])}
          />
          </div>
          {rows.length === lines && (
            <button type="button" class="btn" onClick={() => setLines((l) => l + PAGE_SIZE)}>Load more</button>
          )}
        </>
      )}
    </Panel>
  );
}

function levelColor(l: LogLevel): string {
  return l === "err" ? "var(--danger)" : l === "warn" ? "var(--warn)" : "var(--text-3)";
}

function AuditTab() {
  const [rows, setRows] = useState<AuditEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  function load() {
    getAudit().then((r) => { setRows(r); setError(null); }, (e) => setError(e instanceof Error ? e.message : "Failed to load audit log"));
  }
  useEffect(load, []);

  return (
    <Panel title="Audit" subtitle="Every write action, newest first">
      {error ? (
        <ErrorState message={error} onRetry={load} />
      ) : !rows ? (
        <Loading label="Loading audit log…" />
      ) : rows.length === 0 ? (
        <Empty icon="scroll-text" title="No audit entries yet" />
      ) : (
        <div class="table-scroll">
        <Table
          columns={[{ title: "Time", width: 90 }, { title: "User", width: 100 }, { title: "Action", width: 160 }, { title: "Target", width: 140 }, { title: "Result", width: 80 }, { title: "Detail" }]}
          rows={rows.map((r) => [
            <span class="mono small">{clock(r.ts)}</span>,
            <span class="mono small">{r.user}</span>,
            <span class="mono small">{r.action}</span>,
            <span class="mono small muted">{r.target}</span>,
            <Chip label={r.result} color={r.result === "ok" ? "var(--ok)" : "var(--danger)"} />,
            <span class="muted small">{r.detail ?? "–"}</span>,
          ])}
        />
        </div>
      )}
    </Panel>
  );
}
