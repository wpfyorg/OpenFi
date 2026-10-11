// QoS: shaper state + bufferbloat warning, enforcement flags, the fair-use tier ladder, and
// per-client month usage with the same whitelist toggle action used on Devices.
import { useState } from "preact/hooks";
import { Chip } from "../components/Chip";
import { qosTiers } from "../components/contractExt";
import { Empty } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Table } from "../components/Table";
import { Toggle } from "../components/Toggle";
import { whitelist } from "../components/clientActions";
import { useSnap } from "../context";
import { bytes, mbps } from "../format";
import type { Qos, QosClient, Snapshot } from "../types";

const SECTION = "#F0873C";

export default function QosPage() {
  const { snap, connected } = useSnap();
  const [override, setOverride] = useState<Record<string, boolean>>({});

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader icon="sliders-horizontal" title="QoS" subtitle={statusLine(snap, connected)} />
      {!snap ? (
        <Panel><Empty icon="sliders-horizontal" title="Waiting for the first poll…" /></Panel>
      ) : !snap.qos ? (
        <Panel><Empty icon="sliders-horizontal" title="No QoS data yet" hint="qos-tiers may not be installed or running." /></Panel>
      ) : (
        <>
          <ShaperPanel qos={snap.qos} />
          <TierLadder qos={snap.qos} />
          <ClientUsageTable qos={snap.qos} override={override} setOverride={setOverride} />
        </>
      )}
    </div>
  );
}

function statusLine(snap: Snapshot | null, connected: boolean): string {
  if (!snap) return connected ? "Connecting…" : "Disconnected";
  if (!snap.qos) return "qos-tiers isn't reporting.";
  const { qos } = snap;
  const shaped = qos.shaper_dl_kbit > 0 || qos.shaper_ul_kbit > 0;
  const enforcing = qos.enforcing_burst || qos.enforcing_monthly;
  return `Shaper ${shaped ? `${(qos.shaper_dl_kbit / 1000).toFixed(0)}/${(qos.shaper_ul_kbit / 1000).toFixed(0)} Mbit` : "unlimited"}, ${enforcing ? "enforcing" : "counting only"}.`;
}

function ShaperPanel({ qos }: { qos: Qos }) {
  const unlimited = qos.shaper_dl_kbit === 0 && qos.shaper_ul_kbit === 0;
  return (
    <Panel
      title="Shaper"
      subtitle="Bandwidth caps applied at the gateway."
      right={
        <div class="filters-bar">
          <Chip label={qos.enforcing_burst ? "Burst enforcing" : "Burst counting only"} color={qos.enforcing_burst ? "var(--warn)" : "var(--text-3)"} />
          <Chip label={qos.enforcing_monthly ? "Monthly enforcing" : "Monthly counting only"} color={qos.enforcing_monthly ? "var(--warn)" : "var(--text-3)"} />
        </div>
      }
    >
      <div class="readout">
        <div class="readout-cell">
          <span class="label">Download cap</span>
          <span class="readout-value down">
            <span class="mono">{unlimited ? "Unlimited" : mbps(qos.shaper_dl_kbit * 1000)}</span>
            {!unlimited && <small>Mbps</small>}
          </span>
        </div>
        <div class="readout-cell">
          <span class="label">Upload cap</span>
          <span class="readout-value up">
            <span class="mono">{qos.shaper_ul_kbit === 0 ? "Unlimited" : mbps(qos.shaper_ul_kbit * 1000)}</span>
            {qos.shaper_ul_kbit !== 0 && <small>Mbps</small>}
          </span>
        </div>
      </div>
      {unlimited && (
        <div class="callout warn">
          <span class="mono">!</span>
          <span>The shaper is off (0 = unlimited), so latency can rise under load — this is the classic bufferbloat symptom on a busy line.</span>
        </div>
      )}
    </Panel>
  );
}

function TierLadder({ qos }: { qos: Qos }) {
  const tiers = [...qosTiers(qos)].sort((a, b) => a.min_gb - b.min_gb);
  return (
    <Panel title="Fair-use tiers" subtitle="Monthly ladder from /etc/device-qos.conf, low to high.">
      <div class="tier-list">
        {tiers.map((t) => (
          <div class="tier-row" key={t.id}>
            <span>
              <b>{t.label}</b>{" "}
              <span class="muted small">
                {t.min_gb} GB{t.max_gb != null ? `–${t.max_gb} GB` : "+"}
                {t.rate_mbit != null ? ` · capped at ${t.rate_mbit} Mbit` : ""}
              </span>
            </span>
            <span class="mono">{t.clients} client{t.clients === 1 ? "" : "s"}</span>
          </div>
        ))}
        {tiers.length === 0 && <Empty icon="sliders-horizontal" title="No tiers configured" />}
      </div>
    </Panel>
  );
}

function ClientUsageTable({
  qos, override, setOverride,
}: { qos: Qos; override: Record<string, boolean>; setOverride: (fn: (m: Record<string, boolean>) => Record<string, boolean>) => void }) {
  const rows = [...qos.clients].sort((a, b) => b.month_bytes - a.month_bytes);
  const { snap } = useSnap();
  const nameOf = new Map((snap?.clients ?? []).map((c) => [c.mac, c.hostname]));
  const tierLabel = new Map(qosTiers(qos).map((t) => [t.id, t.label]));
  return (
    <Panel title="Per-client month usage" subtitle="Toggle whitelist to exempt a device from shaping.">
      <div class="table-scroll">
        <Table
          columns={[{ title: "Client" }, { title: "Tier", width: 140 }, { title: "Month total", width: 120, align: "right" }, { title: "Whitelisted", width: 120, align: "right" }]}
          rowKey={(i) => rows[i].mac}
          empty="No clients counted yet"
          rows={rows.map((c: QosClient) => {
            const wl = override[c.mac] ?? c.whitelisted;
            return [
              <div>
                <b>{nameOf.get(c.mac) ?? c.mac}</b>
                {nameOf.get(c.mac) && <div class="mono muted small">{c.mac}</div>}
              </div>,
              <Chip label={wl ? "Whitelisted" : tierLabel.get(c.tier) ?? c.tier} color={wl ? "var(--ok)" : "var(--text-3)"} />,
              <span class="mono">{bytes(c.month_bytes)}</span>,
              <Toggle
                checked={wl}
                onChange={(on) => {
                  setOverride((m) => ({ ...m, [c.mac]: on }));
                  whitelist(c.mac, c.mac, on, () => setOverride((m) => ({ ...m, [c.mac]: !on })));
                }}
              />,
            ];
          })}
        />
      </div>
    </Panel>
  );
}
