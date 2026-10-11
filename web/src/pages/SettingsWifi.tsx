// Multi-node WiFi network editor + apply-plan flow.
// GET /wifi/networks, POST /wifi/networks/{id}/plan, POST /wifi/plans/{id}/apply, GET /wifi/plans/{id}.
import { useEffect, useState } from "preact/hooks";
import { ApiError, applyWifiPlan, getWifiPlanStatus, getWifiNetworks, planWifiNetwork } from "../api";
import { ApplyPlan } from "../components/ApplyPlan";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { ConfirmDialog } from "../components/Dialog";
import { Empty, ErrorState, Loading } from "../components/States";
import { Icon } from "../components/icons";
import { NodeTag } from "../components/NodeTag";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Table } from "../components/Table";
import { toast } from "../components/Toast";
import { Toggle } from "../components/Toggle";
import { useSnap } from "../context";
import type { Band, Plan, PlanStatus, Pmf, WifiNetwork, WifiPlanRequest, WifiRoaming, WifiSecurity } from "../types";

const SECTION = "#3CC4D8";
const SECURITY_LABEL: Record<WifiSecurity, string> = {
  "sae-mixed": "WPA2/WPA3 (SAE mixed)", psk2: "WPA2-PSK", sae: "WPA3 (SAE)", none: "Open",
};
const BANDS: Band[] = ["2.4", "5"];

export default function SettingsWifi() {
  const { snap } = useSnap();
  const [networks, setNetworks] = useState<WifiNetwork[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  function load() {
    setError(null);
    getWifiNetworks().then(
      (rows) => { setNetworks(rows); setSelectedId((id) => id ?? rows[0]?.id ?? null); },
      (e) => setError(e instanceof Error ? e.message : "Failed to load WiFi networks"),
    );
  }
  useEffect(load, []);

  const selected = networks?.find((n) => n.id === selectedId) ?? null;
  const totalClients = networks?.reduce((a, n) => a + n.clients, 0) ?? 0;

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader
        icon="settings"
        title="WiFi networks"
        subtitle={networks ? `${networks.length} SSID${networks.length === 1 ? "" : "s"}, ${totalClients} client${totalClients === 1 ? "" : "s"} connected` : "One definition, pushed to every node"}
        right={<>
          <Button icon="network" onClick={() => { location.hash = "#/settings/nodes"; }}>Nodes</Button>
          <Button icon="plus" disabled title="Adding a new network isn't supported by the API yet">New network</Button>
        </>}
      />
      {error ? (
        <ErrorState message={error} onRetry={load} />
      ) : !networks ? (
        <Panel><Loading label="Loading networks…" /></Panel>
      ) : (
        <div class="row wifi-settings-row">
          <Panel title="Networks" subtitle={`${networks.length} SSID${networks.length === 1 ? "" : "s"} across ${snap?.nodes.length ?? "–"} nodes · uci show wireless`}>
            <NetworksTable networks={networks} selectedId={selectedId} onSelect={setSelectedId} />
          </Panel>
          {selected && (
            <NetworkEditor
              key={selected.id}
              network={selected}
              onApplied={load}
            />
          )}
        </div>
      )}
    </div>
  );
}

function NetworksTable({ networks, selectedId, onSelect }: { networks: WifiNetwork[]; selectedId: string | null; onSelect: (id: string) => void }) {
  return (
    <div class="table-scroll">
    <Table
      columns={[{ title: "Network" }, { title: "Security & band" }, { title: "Broadcast on" }, { title: "Clients / roaming", align: "right" }]}
      empty="No WiFi networks reported"
      rows={networks.map((n) => [
        <button type="button" class={`row-select-btn ${n.id === selectedId ? "on" : ""}`} onClick={() => onSelect(n.id)}>
          <div class="wifi-net-title">
            {n.locked && <Icon name="lock" size={12} color="var(--text-3)" />}
            <b>{n.ssid}</b>
          </div>
          <div class="muted small">{purposeLabel(n.purpose)}</div>
        </button>,
        <div class="wifi-secband">
          <Chip label={SECURITY_LABEL[n.security]} />
          <div class="mono small muted">{n.bands.map((b) => `${b} GHz`).join(" + ")}</div>
        </div>,
        <div class="wifi-bc-chips">
          {Object.entries(n.broadcast).map(([node, bc]) => {
            const on = bc["2.4"] || bc["5"];
            if (!on) return <span class="node-tag dim" key={node}><span class="node-tag-sw" style={{ background: "var(--text-3)", height: 12 }} /><span class="mono small">{node} off</span></span>;
            return <NodeTag node={node} size={11} key={node} />;
          })}
        </div>,
        <div class="wifi-clients-roam">
          <b class="mono">{n.clients} client{n.clients === 1 ? "" : "s"}</b>
          <span class="muted small">{roamingSummary(n)}</span>
        </div>,
      ])}
    />
    </div>
  );
}

function purposeLabel(p: WifiNetwork["purpose"]): string {
  return p === "primary" ? "Primary" : p === "iot" ? "IoT" : "Backhaul";
}

function roamingSummary(n: WifiNetwork): string {
  if (n.locked) return "system-managed";
  const bits: string[] = [];
  if (n.roaming.rrm || n.roaming.btm) bits.push("k/v");
  if (n.roaming.ft || n.roaming.ft_over_ds) bits.push("r");
  if (n.roaming.usteer) bits.push("usteer");
  return bits.length ? bits.join(" · ") : "not steered";
}

// ---- editor state ----

interface RoamingEdit extends WifiRoaming {}

function cloneRoaming(r: WifiRoaming): RoamingEdit { return { ...r }; }

function NetworkEditor({ network, onApplied }: { network: WifiNetwork; onApplied: () => void }) {
  const { snap } = useSnap();
  const [ssid, setSsid] = useState(network.ssid);
  const [key, setKey] = useState("");
  const [revealKey, setRevealKey] = useState(false);
  const [security, setSecurity] = useState<WifiSecurity>(network.security);
  const [broadcast, setBroadcast] = useState<Record<string, { "2.4": boolean | null; "5": boolean | null }>>(() => deepCloneBroadcast(network.broadcast));
  const [roaming, setRoaming] = useState<RoamingEdit>(() => cloneRoaming(network.roaming));

  const [plan, setPlan] = useState<Plan | null>(null);
  const [planStatus, setPlanStatus] = useState<PlanStatus | null>(null);
  const [planning, setPlanning] = useState(false);
  const [applying, setApplying] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [planError, setPlanError] = useState<string | null>(null);

  useEffect(() => {
    if (!plan || !planStatus || planStatus.state !== "running") return;
    const id = setInterval(() => {
      getWifiPlanStatus(plan.id).then(setPlanStatus, () => {});
    }, 1000);
    return () => clearInterval(id);
  }, [plan, planStatus?.state]);

  useEffect(() => {
    if (planStatus?.state === "done") { toast(`WiFi changes applied to ${network.ssid}.`, "success"); onApplied(); }
    if (planStatus?.state === "failed") toast(`Applying changes to ${network.ssid} failed.`, "error");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [planStatus?.state]);

  const nodeOrder = snap?.nodes.map((n) => n.name).filter((n) => n in network.broadcast) ?? Object.keys(network.broadcast);

  const ssidChanged = ssid !== network.ssid;
  const keyChanged = key.length > 0;
  const securityChanged = security !== network.security;
  const broadcastDiff = diffBroadcast(network.broadcast, broadcast);
  const roamingDiff = diffRoaming(network.roaming, roaming);
  const hasChanges = ssidChanged || keyChanged || securityChanged || Object.keys(broadcastDiff).length > 0 || Object.keys(roamingDiff).length > 0;

  function resetChanges() {
    setSsid(network.ssid); setKey(""); setSecurity(network.security);
    setBroadcast(deepCloneBroadcast(network.broadcast)); setRoaming(cloneRoaming(network.roaming));
    setPlan(null); setPlanStatus(null); setPlanError(null);
  }

  async function review() {
    setPlanning(true);
    setPlanError(null);
    const body: WifiPlanRequest = {};
    if (ssidChanged) body.ssid = ssid;
    if (keyChanged) body.key = key;
    if (securityChanged) body.security = security;
    if (Object.keys(broadcastDiff).length) body.broadcast = broadcastDiff;
    if (Object.keys(roamingDiff).length) body.roaming = roamingDiff;
    try {
      const p = await planWifiNetwork(network.id, body);
      setPlan(p);
      setPlanStatus(null);
    } catch (e) {
      setPlanError(e instanceof ApiError ? e.message : "Failed to build a plan for this change");
    } finally {
      setPlanning(false);
    }
  }

  const offlineTargets = plan ? plan.order.filter((n) => snap?.nodes.find((sn) => sn.name === n)?.online === false) : [];
  const canApply = !!plan && offlineTargets.length === 0 && (!planStatus || planStatus.state !== "running");

  async function doApply() {
    if (!plan) return;
    setApplying(true);
    try {
      await applyWifiPlan(plan.id);
      setPlanStatus(await getWifiPlanStatus(plan.id));
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to start applying the plan", "error");
    } finally {
      setApplying(false);
    }
  }

  const locked = network.locked;

  return (
    <Panel class="panel-wifi-editor" title="Edit network" subtitle={`${network.ssid} · applies to ${nodeOrder.length} node${nodeOrder.length === 1 ? "" : "s"}`}
      right={<Chip label="selected" color="var(--section)" />}
    >
      {locked ? (
        <Empty icon="lock" title="System-managed" hint="The mesh backhaul network is configured automatically and can't be edited here." />
      ) : (
        <>
          <details class="wifi-section" open>
            <summary>Basics</summary>
            <div class="field">
              <label for="wifi-ssid">Network name</label>
              <input id="wifi-ssid" value={ssid} onInput={(e) => setSsid((e.target as HTMLInputElement).value)} />
            </div>
            <div class="field">
              <label for="wifi-key">Password</label>
              <div class="password-field">
                <input
                  id="wifi-key" type={revealKey ? "text" : "password"} value={key}
                  onInput={(e) => setKey((e.target as HTMLInputElement).value)}
                  placeholder="Leave blank to keep the current password"
                />
                <button type="button" class="icon-btn" onClick={() => setRevealKey((v) => !v)} aria-label={revealKey ? "Hide password" : "Show password"}>
                  <Icon name={revealKey ? "eye-off" : "eye"} size={15} />
                </button>
              </div>
            </div>
            <div class="field">
              <label for="wifi-security">Security</label>
              <select id="wifi-security" value={security} onChange={(e) => setSecurity((e.target as HTMLSelectElement).value as WifiSecurity)}>
                {(Object.keys(SECURITY_LABEL) as WifiSecurity[]).map((s) => <option value={s} key={s}>{SECURITY_LABEL[s]}</option>)}
              </select>
            </div>
          </details>

          <details class="wifi-section" open>
            <summary>Where it broadcasts</summary>
            <BroadcastMatrix nodeOrder={nodeOrder} broadcast={broadcast} onChange={setBroadcast} />
          </details>

          <details class="wifi-section">
            <summary>Roaming</summary>
            <RoamingFields roaming={roaming} onChange={setRoaming} />
          </details>

          <details class="wifi-section" open>
            <summary>Review and apply</summary>
            <div class="review-apply">
              {!hasChanges && <p class="muted small">No changes yet. Edit the fields above, then review.</p>}
              <div class="review-actions">
                <Button onClick={resetChanges} disabled={!hasChanges && !plan}>Cancel</Button>
                <Button primary onClick={review} disabled={!hasChanges || planning}>{planning ? "Building plan…" : "Review changes"}</Button>
              </div>
              {planError && <p class="field-error">{planError}</p>}
              {plan && (
                <>
                  <ApplyPlan plan={plan} status={planStatus} />
                  {offlineTargets.length > 0 && (
                    <p class="field-error">{offlineTargets.join(", ")} {offlineTargets.length === 1 ? "is" : "are"} offline. Reconnect it before applying.</p>
                  )}
                  <div class="review-actions">
                    <Button
                      primary icon="refresh-cw"
                      disabled={!canApply || applying || planStatus?.state === "running"}
                      onClick={() => setConfirmOpen(true)}
                    >
                      {planStatus?.state === "running" ? "Applying…" : `Apply to ${plan.order.length} node${plan.order.length === 1 ? "" : "s"}`}
                    </Button>
                  </div>
                </>
              )}
            </div>
          </details>
        </>
      )}
      {confirmOpen && plan && (
        <ConfirmDialog
          title="Apply WiFi changes?"
          confirmLabel="Apply"
          onClose={() => setConfirmOpen(false)}
          onConfirm={doApply}
          body={
            <p>
              Changes go to {plan.order.map((n, i) => <span key={n}><b class="mono">{n}</b>{i < plan.order.length - 1 ? ", then " : ""}</span>)}.
              Each node reverts on its own after 30 s if it drops off.
            </p>
          }
        />
      )}
    </Panel>
  );
}

function deepCloneBroadcast(bc: WifiNetwork["broadcast"]): WifiNetwork["broadcast"] {
  const out: WifiNetwork["broadcast"] = {};
  for (const [node, v] of Object.entries(bc)) out[node] = { ...v };
  return out;
}

function diffBroadcast(
  original: WifiNetwork["broadcast"], edited: WifiNetwork["broadcast"],
): Record<string, { "2.4"?: boolean; "5"?: boolean }> {
  const out: Record<string, { "2.4"?: boolean; "5"?: boolean }> = {};
  for (const node of Object.keys(edited)) {
    const o = original[node], e = edited[node];
    const nodeDiff: { "2.4"?: boolean; "5"?: boolean } = {};
    for (const band of BANDS) {
      if (o?.[band] != null && e[band] != null && o[band] !== e[band]) nodeDiff[band] = e[band] as boolean;
    }
    if (Object.keys(nodeDiff).length) out[node] = nodeDiff;
  }
  return out;
}

function diffRoaming(original: WifiRoaming, edited: WifiRoaming): Partial<WifiRoaming> {
  const out: Partial<WifiRoaming> = {};
  (Object.keys(edited) as (keyof WifiRoaming)[]).forEach((k) => {
    if (original[k] !== edited[k]) (out as Record<string, unknown>)[k] = edited[k];
  });
  return out;
}

function BroadcastMatrix({
  nodeOrder, broadcast, onChange,
}: { nodeOrder: string[]; broadcast: WifiNetwork["broadcast"]; onChange: (b: WifiNetwork["broadcast"]) => void }) {
  function flip(node: string, band: Band) {
    const cur = broadcast[node]?.[band];
    if (cur == null) return;
    onChange({ ...broadcast, [node]: { ...broadcast[node], [band]: !cur } });
  }
  const disabledNotes = nodeOrder.filter((n) => broadcast[n] && (broadcast[n]["2.4"] == null || broadcast[n]["5"] == null));
  return (
    <div class="broadcast-matrix">
      <div class="bc-row bc-head">
        <span>Node</span><span>2.4 GHz</span><span>5 GHz</span>
      </div>
      {nodeOrder.map((node) => {
        const bc = broadcast[node] ?? { "2.4": null, "5": null };
        return (
          <div class="bc-row" key={node}>
            <NodeTag node={node} size={12} />
            <Toggle checked={!!bc["2.4"]} disabled={bc["2.4"] == null} onChange={() => flip(node, "2.4")} />
            <Toggle checked={!!bc["5"]} disabled={bc["5"] == null} onChange={() => flip(node, "5")} />
          </div>
        );
      })}
      {disabledNotes.length > 0 && (
        <p class="muted small">Radio off or absent on {disabledNotes.join(", ")} — can't be toggled here.</p>
      )}
    </div>
  );
}

function RoamingFields({ roaming, onChange }: { roaming: RoamingEdit; onChange: (r: RoamingEdit) => void }) {
  const kv = roaming.rrm && roaming.btm;
  return (
    <div class="roaming-fields">
      <label class="switch-row">
        <div>
          <span class="switch-title">802.11r FT-over-DS</span>
          <span class="muted small">Fast, seamless roam between nodes</span>
        </div>
        <Toggle checked={roaming.ft_over_ds} onChange={(v) => onChange({ ...roaming, ft: v || roaming.ft, ft_over_ds: v })} />
      </label>
      <div class="field">
        <label for="wifi-mobility-domain">Mobility domain</label>
        <input
          id="wifi-mobility-domain" class="mono" value={roaming.mobility_domain ?? ""} disabled={!roaming.ft_over_ds}
          onInput={(e) => onChange({ ...roaming, mobility_domain: (e.target as HTMLInputElement).value || null })}
          placeholder="e.g. 2c3c" maxLength={4}
        />
      </div>
      <label class="switch-row">
        <div>
          <span class="switch-title">802.11k/v neighbor reports</span>
          <span class="muted small">Helps clients pick the best node before roaming</span>
        </div>
        <Toggle checked={kv} onChange={(v) => onChange({ ...roaming, rrm: v, btm: v })} />
      </label>
      <div class="field">
        <label for="wifi-pmf">802.11w (PMF)</label>
        <select id="wifi-pmf" value={roaming.pmf} onChange={(e) => onChange({ ...roaming, pmf: (e.target as HTMLSelectElement).value as Pmf })}>
          <option value="off">Off</option>
          <option value="optional">Optional</option>
          <option value="required">Required</option>
        </select>
      </div>
      <label class="switch-row">
        <div>
          <span class="switch-title">usteer band steering</span>
          <span class="muted small">Nudges clients to the strongest node and band</span>
        </div>
        <Toggle checked={roaming.usteer} onChange={(v) => onChange({ ...roaming, usteer: v })} />
      </label>
    </div>
  );
}
