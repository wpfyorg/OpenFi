// Side panel for a selected topology node: identity, health meters, radios, links and node actions.
// Used by pages/Topology.tsx. Actions call the existing /nodes/{n}/... routes from api.ts.
import { useState } from "preact/hooks";
import { patchRadio, reloadWifi, rebootNode } from "../api";
import { toast } from "./Toast";
import { Dialog } from "./Dialog";
import { Button } from "./Button";
import { Icon } from "./icons";
import { NodeTag } from "./NodeTag";
import { ShareBar } from "./ShareBar";
import { celsius, uptime } from "../format";
import { nodeColor } from "../tokens";
import type { Node, Radio as RadioT, Temp } from "../types";

export interface NodeCardProps {
  node: Node;
  /** WAN details, shown only for the gateway node (it's the one with the WAN link). */
  wanIp?: string | null;
  onOpenShell: (node: string) => void;
}

/** Node identity + health + radios + links + actions. The right-hand panel on the Topology page. */
export function NodeCard({ node, wanIp, onOpenShell }: NodeCardProps) {
  const [rebootOpen, setRebootOpen] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  const cpu = Math.min(99, Math.round(node.load[0] * 100));
  const memUsed = node.mem_total - node.mem_available;
  const memPct = node.mem_total > 0 ? Math.round((memUsed / node.mem_total) * 100) : 0;
  const overlayPct = node.overlay_used != null && node.overlay_total ? Math.round((node.overlay_used / node.overlay_total) * 100) : null;
  const hottest = node.temps.length ? Math.max(...node.temps.map((t) => t.celsius)) : null;
  const isHot = (t: Temp) => t.celsius >= 70;

  const clientsByBand = new Map<string, number>();
  for (const r of node.radios) clientsByBand.set(r.band, (clientsByBand.get(r.band) ?? 0) + r.clients);
  const totalClients = node.radios.reduce((a, r) => a + r.clients, 0);
  const bandSummary = [...clientsByBand.entries()].filter(([, n]) => n > 0).map(([b, n]) => `${n} × ${b}G`).join(", ");

  async function restartWifi() {
    setBusy("wifi");
    try {
      await reloadWifi(node.name);
      toast(`Restarting WiFi on ${node.name}…`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to restart WiFi", "error");
    } finally {
      setBusy(null);
    }
  }

  async function doReboot() {
    setBusy("reboot");
    try {
      await rebootNode(node.name);
      toast(`Rebooting ${node.name}…`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to reboot", "error");
    } finally {
      setBusy(null);
    }
  }

  return (
    <section class="panel node-card" aria-label={`${node.name} details`}>
      <header class="node-card-head">
        <span class="node-card-tile" style={{ background: node.online ? nodeColor(node.name) : "var(--text-3)" }}>
          <Icon name={node.role === "gateway" ? "router" : "wifi"} size={18} color="#fff" />
        </span>
        <div class="node-card-titles">
          <h2 class="mono">{node.name}</h2>
          <p>{node.model ?? "Unknown model"}</p>
        </div>
      </header>

      <div class="node-card-chips">
        <span class={`chip ${node.online ? "chip-ok" : "chip-bad"}`}>
          <span class="chip-dot" style={{ background: node.online ? "var(--ok)" : "var(--danger)" }} />
          {node.online ? `online ${uptime(node.uptime)}` : "offline"}
        </span>
        <span class="chip">{node.role}</span>
        {node.error && <span class="chip chip-bad"><Icon name="triangle-alert" size={11} color="var(--danger)" />{node.error}</span>}
      </div>

      <dl class="node-card-facts">
        <div><dt>Host</dt><dd class="mono">{node.host}</dd></div>
        {node.role === "gateway" && wanIp && <div><dt>WAN</dt><dd class="mono">{wanIp}</dd></div>}
        <div><dt>Firmware</dt><dd class="mono">{node.firmware ?? "–"}</dd></div>
        <div><dt>Clients</dt><dd class="mono">{totalClients}{bandSummary ? ` (${bandSummary})` : ""}</dd></div>
      </dl>

      <div class="node-card-meters">
        <Meter label="CPU" value={cpu} sub={`load ${node.load[0].toFixed(2)}`} />
        <Meter label="Memory" value={memPct} sub={`${Math.round(memUsed / 1e6)} / ${Math.round(node.mem_total / 1e6)} MB`} />
        {overlayPct != null && <Meter label="Overlay" value={overlayPct} />}
      </div>

      {node.temps.length > 0 && (
        <div class="node-card-section">
          <h3>Temperatures{hottest != null && hottest >= 70 ? <span class="hot-flag">hot at {celsius(hottest)}</span> : null}</h3>
          {node.temps.map((t) => (
            <div class="temp-row" key={t.label}>
              <span class="label">{t.label}</span>
              <ShareBar frac={Math.min(1, t.celsius / (t.crit ?? 100))} color={isHot(t) ? "var(--orange)" : "var(--ok)"} width={90} />
              <span class={isHot(t) ? "mono small hot" : "mono small"}>{celsius(t.celsius)}</span>
            </div>
          ))}
        </div>
      )}

      {node.radios.length > 0 && (
        <div class="node-card-section">
          <h3>Radios</h3>
          {node.radios.map((r) => <RadioRow key={r.name} radio={r} node={node.name} />)}
        </div>
      )}

      {node.links.length > 0 && (
        <div class="node-card-section">
          <h3>Links</h3>
          {node.links.map((l, i) => (
            <div class="link-row" key={`${l.kind}-${l.peer}-${i}`}>
              <span class={`dot ${l.quality === "strong" ? "ok" : l.quality === "fair" ? "warn" : "bad"}`} />
              <span class="mono small">{l.kind} · <NodeTag node={l.peer} size={11} /></span>
              <span class="mono small muted">
                {l.signal_dbm != null ? `${l.signal_dbm} dBm` : ""}
                {l.rx_mbps != null ? ` · ${Math.round(l.rx_mbps)} Mbps` : ""}
              </span>
            </div>
          ))}
        </div>
      )}

      <div class="node-card-actions">
        <Button icon="refresh-cw" onClick={restartWifi} disabled={!node.online || busy !== null} title={!node.online ? "Node is offline" : undefined}>
          Restart WiFi
        </Button>
        <Button icon="power" onClick={() => setRebootOpen(true)} disabled={!node.online || busy !== null}>
          Reboot
        </Button>
        <a class="btn" href={`http://${node.host}/cgi-bin/luci`} target="_blank" rel="noopener noreferrer">
          <Icon name="external-link" size={13} /> Open LuCI
        </a>
        <Button icon="terminal" primary onClick={() => onOpenShell(node.name)} disabled={!node.online}>
          Shell
        </Button>
      </div>

      {rebootOpen && (
        <RebootConfirm nodeName={node.name} onClose={() => setRebootOpen(false)} onConfirm={doReboot} />
      )}
    </section>
  );
}

function Meter({ label, value, sub }: { label: string; value: number; sub?: string }) {
  return (
    <div class="node-card-meter">
      <div class="meter-head"><span class="label">{label}</span><span class="mono small">{value}%</span></div>
      <ShareBar frac={value / 100} color={value > 85 ? "var(--danger)" : value > 60 ? "var(--warn)" : "var(--section)"} width="100%" />
      {sub && <span class="mono small muted">{sub}</span>}
    </div>
  );
}

function RadioRow({ radio, node }: { radio: RadioT; node: string }) {
  const [busy, setBusy] = useState(false);
  async function toggleDisabled() {
    setBusy(true);
    try {
      await patchRadio(node, radio.name, { disabled: !radio.disabled });
      toast(`${radio.disabled ? "Enabling" : "Disabling"} ${radio.name} on ${node}…`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to update radio", "error");
    } finally {
      setBusy(false);
    }
  }
  return (
    <div class="radio-row">
      <Icon name="radio" size={13} color="var(--text-3)" />
      <span class="mono small">{radio.band} GHz</span>
      <span class="mono small muted">{radio.disabled ? "disabled" : `ch ${radio.channel ?? "–"} ${radio.htmode ?? ""}`}</span>
      <span class="mono small muted">{radio.disabled ? "" : `${radio.txpower_dbm ?? "–"} dBm`}</span>
      <span class="mono small radio-row-clients">{radio.clients}</span>
      <button type="button" class="btn btn-tiny" onClick={toggleDisabled} disabled={busy}>
        {radio.disabled ? "Enable" : "Disable"}
      </button>
    </div>
  );
}

/** Reboot requires typing the node's name, since it's disruptive and immediate. */
function RebootConfirm({ nodeName, onClose, onConfirm }: { nodeName: string; onClose: () => void; onConfirm: () => void }) {
  const [text, setText] = useState("");
  const match = text.trim() === nodeName;
  return (
    <Dialog
      title={`Reboot ${nodeName}?`}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <button
            type="button"
            class="btn btn-danger"
            disabled={!match}
            onClick={() => { onConfirm(); onClose(); }}
          >
            Reboot node
          </button>
        </>
      }
    >
      <p>This immediately restarts <b class="mono">{nodeName}</b>. Clients on it will briefly drop.</p>
      <p>Type <b class="mono">{nodeName}</b> to confirm.</p>
      <input
        class="text-input"
        value={text}
        onInput={(e) => setText((e.target as HTMLInputElement).value)}
        placeholder={nodeName}
        aria-label={`Type ${nodeName} to confirm`}
        autoFocus
      />
    </Dialog>
  );
}
