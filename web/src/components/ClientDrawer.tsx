// Client detail drawer: opened by clicking a row in the Devices table. Shows connection detail and
// month-to-date usage, and wires Kick / Reserve IP / Whitelist through ConfirmDialog + the shared
// clientActions helpers (toast on success/error, optimistic whitelist toggle).
import { useState } from "preact/hooks";
import { ConfirmDialog, Dialog } from "./Dialog";
import { NodeTag } from "./NodeTag";
import { Toggle } from "./Toggle";
import { Button } from "./Button";
import { Icon } from "./icons";
import { kick, reserve, whitelist } from "./clientActions";
import { bitrate, bytes, dbm, relTime } from "../format";
import type { Client, ClientUsage } from "../types";

const BAN_OPTIONS: { value: number; label: string }[] = [
  { value: 60, label: "1 minute" },
  { value: 600, label: "10 minutes" },
  { value: 3600, label: "1 hour" },
  { value: 86400, label: "24 hours" },
];

export function ClientDrawer({
  client, usage, whitelisted, onWhitelistChange, onClose,
}: {
  client: Client; usage: ClientUsage | null; whitelisted: boolean;
  onWhitelistChange: (on: boolean) => void; onClose: () => void;
}) {
  const [confirmKick, setConfirmKick] = useState(false);
  const [banSecs, setBanSecs] = useState(600);
  const [reserving, setReserving] = useState(false);
  const [ip, setIp] = useState(client.ip ?? "");
  const [name, setName] = useState(client.hostname ?? "");
  const label = client.hostname ?? client.mac;

  return (
    <Dialog title={label} onClose={onClose}>
      <div class="drawer-section">
        <div class="drawer-row"><span class="muted">MAC address</span><span class="mono">{client.mac}</span></div>
        <div class="drawer-row"><span class="muted">IP address</span><span class="mono">{client.ip ?? "n/a"}</span></div>
        <div class="drawer-row">
          <span class="muted">Connection</span>
          <span>
            {client.wired
              ? <span class="mono">Wired{client.node ? <> via <NodeTag node={client.node} /></> : ""}</span>
              : client.node
                ? <span class="mono"><NodeTag node={client.node} /> {client.band ? `${client.band} GHz` : ""}{client.signal_dbm != null ? `, ${dbm(client.signal_dbm)}` : ""}</span>
                : "Offline"}
          </span>
        </div>
        {(client.rx_rate != null || client.tx_rate != null) && (
          <div class="drawer-row">
            <span class="muted">PHY rate</span>
            <span class="mono">{client.rx_rate != null ? bitrate(client.rx_rate) : "n/a"} down / {client.tx_rate != null ? bitrate(client.tx_rate) : "n/a"} up</span>
          </div>
        )}
        <div class="drawer-row"><span class="muted">Last seen</span><span class="mono">{client.online ? "Online now" : relTime(client.last_seen)}</span></div>
        <div class="drawer-row"><span class="muted">Reserved IP</span><span>{client.reserved ? "Yes" : "No"}</span></div>
      </div>

      <div class="drawer-section">
        <div class="drawer-row">
          <span class="muted">This month</span>
          <span class="mono">{usage ? `${bytes(usage.rx_bytes)} down, ${bytes(usage.tx_bytes)} up` : "No usage data yet"}</span>
        </div>
        {usage && usage.top_apps.length > 0 && (
          <div class="drawer-row"><span class="muted">Top apps</span><span>{usage.top_apps.slice(0, 3).join(", ")}</span></div>
        )}
      </div>

      <div class="drawer-section drawer-actions">
        <label class="toggle" style={{ justifyContent: "space-between" }}>
          <span class="toggle-label">Whitelist (exempt from fair-use shaping)</span>
          <Toggle
            checked={whitelisted}
            onChange={(on) => {
              onWhitelistChange(on);
              whitelist(client.mac, label, on, () => onWhitelistChange(!on));
            }}
          />
        </label>
        <Button icon="id-card" onClick={() => { setIp(client.ip ?? ""); setName(client.hostname ?? ""); setReserving(true); }}>
          Reserve IP
        </Button>
        <Button icon="ban" onClick={() => setConfirmKick(true)} disabled={!client.online || client.wired}
          title={client.wired ? "Wired clients can't be deauthenticated" : undefined}>
          Kick from WiFi
        </Button>
      </div>

      {confirmKick && (
        <ConfirmDialog
          title={`Kick ${label}?`}
          danger
          confirmLabel="Kick"
          onClose={() => setConfirmKick(false)}
          onConfirm={() => { kick(client.mac, label, { ban_secs: banSecs }); }}
          body={
            <div class="field">
              <p>Disconnects the client from its access point. It can reassociate immediately unless banned.</p>
              <label for="ban-secs">Ban for</label>
              <select id="ban-secs" value={banSecs} onChange={(e) => setBanSecs(Number((e.target as HTMLSelectElement).value))}>
                {BAN_OPTIONS.map((o) => <option value={o.value}>{o.label}</option>)}
              </select>
            </div>
          }
        />
      )}

      {reserving && (
        <Dialog title={`Reserve an IP for ${label}`} onClose={() => setReserving(false)} footer={
          <>
            <Button onClick={() => setReserving(false)}>Cancel</Button>
            <button type="button" class="btn btn-primary" onClick={() => { reserve(client.mac, label, { ip, name }); setReserving(false); }}>
              <Icon name="id-card" size={13} /> Reserve
            </button>
          </>
        }>
          <div class="field">
            <label for="reserve-ip">IP address</label>
            <input id="reserve-ip" value={ip} onInput={(e) => setIp((e.target as HTMLInputElement).value)} />
          </div>
          <div class="field">
            <label for="reserve-name">Hostname</label>
            <input id="reserve-name" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
          </div>
        </Dialog>
      )}
    </Dialog>
  );
}
