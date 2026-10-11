import { useEffect, useState } from "preact/hooks";
import {
  ApiError, addConfiguredNode, getConfiguredNodes, removeConfiguredNode,
  type AddNodeRequest, type ConfiguredNode,
} from "../api";
import { Button } from "../components/Button";
import { Chip } from "../components/Chip";
import { ConfirmDialog, Dialog } from "../components/Dialog";
import { ErrorState, Loading } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { Table } from "../components/Table";
import { toast } from "../components/Toast";
import { useSnap } from "../context";

const SECTION = "#3CC4D8";

export default function SettingsNodes() {
  const { snap } = useSnap();
  const [nodes, setNodes] = useState<ConfiguredNode[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [removeNode, setRemoveNode] = useState<ConfiguredNode | null>(null);
  const [removing, setRemoving] = useState(false);

  function load() {
    setError(null);
    getConfiguredNodes().then(setNodes, (e) => setError(e instanceof Error ? e.message : "Failed to load nodes"));
  }
  useEffect(load, []);

  async function doRemove(node: ConfiguredNode) {
    setRemoving(true);
    try {
      await removeConfiguredNode(node.name);
      setNodes((rows) => rows?.filter((n) => n.name !== node.name) ?? null);
      toast(`${node.name} removed. OpenWrtFi is restarting…`, "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : "Failed to remove node", "error");
    } finally {
      setRemoving(false);
    }
  }

  return (
    <div class="page" style={{ "--section": SECTION }}>
      <PageHeader
        icon="network"
        title="Nodes"
        subtitle={nodes ? `${nodes.length} configured node${nodes.length === 1 ? "" : "s"}` : "Access points managed by OpenWrtFi"}
        right={<>
          <Button onClick={() => { location.hash = "#/settings/wifi"; }}>WiFi networks</Button>
          <Button primary icon="plus" onClick={() => setAddOpen(true)}>Add AP</Button>
        </>}
      />

      {error ? (
        <ErrorState message={error} onRetry={load} />
      ) : !nodes ? (
        <Panel><Loading label="Loading nodes…" /></Panel>
      ) : (
        <Panel title="Configured nodes" subtitle="Changes are saved to /etc/openwrtfi.json and applied automatically">
          <div class="table-scroll">
            <Table
              columns={[
                { title: "Node" },
                { title: "Address" },
                { title: "Backhaul" },
                { title: "Status" },
                { title: "", align: "right" },
              ]}
              rowKey={(i) => nodes[i].name}
              rows={nodes.map((node) => {
                const live = snap?.nodes.find((n) => n.name === node.name);
                return [
                  <div>
                    <b>{node.name}</b>
                    <div class="muted small">{node.gateway ? "Gateway" : `rpcd · ${node.username}${node.password_set ? " · password" : " · no password"}`}</div>
                  </div>,
                  <span class="mono">{node.host}</span>,
                  <span>{node.gateway ? "Local" : node.wired_uplink ? `Wired · ${node.wired_uplink}` : "Mesh / wireless"}</span>,
                  live ? <Chip label={live.online ? "online" : "offline"} color={live.online ? "var(--ok)" : "var(--danger)"} /> : <Chip label="pending" />,
                  node.gateway ? (
                    <span class="muted small">Required</span>
                  ) : (
                    <button type="button" class="btn btn-danger" disabled={removing} onClick={() => setRemoveNode(node)}>Remove</button>
                  ),
                ];
              })}
            />
          </div>
        </Panel>
      )}

      {addOpen && (
        <AddNodeDialog
          onClose={() => setAddOpen(false)}
          onAdded={(node) => {
            setNodes((rows) => rows ? [...rows, node] : [node]);
            setAddOpen(false);
            toast(`${node.name} added. OpenWrtFi is restarting…`, "success");
          }}
        />
      )}
      {removeNode && (
        <ConfirmDialog
          title={`Remove ${removeNode.name}?`}
          body={<p>This stops OpenWrtFi from polling and managing <b>{removeNode.name}</b>. Nothing is changed on the access point itself.</p>}
          confirmLabel="Remove AP"
          danger
          onClose={() => setRemoveNode(null)}
          onConfirm={() => { void doRemove(removeNode); }}
        />
      )}
    </div>
  );
}

function AddNodeDialog({ onClose, onAdded }: { onClose: () => void; onAdded: (node: ConfiguredNode) => void }) {
  const [name, setName] = useState("");
  const [host, setHost] = useState("");
  const [username, setUsername] = useState("root");
  const [password, setPassword] = useState("");
  const [wiredUplink, setWiredUplink] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: Event) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    const body: AddNodeRequest = {
      name: name.trim(),
      host: host.trim(),
      username: username.trim(),
      password,
    };
    if (wiredUplink.trim()) body.wired_uplink = wiredUplink.trim();
    try {
      onAdded(await addConfiguredNode(body));
    } catch (e) {
      setError(e instanceof ApiError ? e.message : e instanceof Error ? e.message : "Failed to add AP");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      title="Add access point"
      onClose={onClose}
      footer={<>
        <Button onClick={onClose} disabled={busy}>Cancel</Button>
        <Button primary onClick={() => (document.getElementById("add-node-form") as HTMLFormElement | null)?.requestSubmit()} disabled={busy || !name.trim() || !host.trim() || !username.trim()}>
          {busy ? "Adding…" : "Add AP"}
        </Button>
      </>}
    >
      <form id="add-node-form" onSubmit={submit}>
        <div class="field">
          <label for="node-name">Name</label>
          <input id="node-name" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} placeholder="ap-5" autoFocus />
        </div>
        <div class="field">
          <label for="node-host">IP address</label>
          <input id="node-host" value={host} onInput={(e) => setHost((e.target as HTMLInputElement).value)} placeholder="192.0.2.6" inputMode="numeric" />
        </div>
        <div class="field">
          <label for="node-user">rpcd username</label>
          <input id="node-user" value={username} onInput={(e) => setUsername((e.target as HTMLInputElement).value)} autoComplete="username" />
        </div>
        <div class="field">
          <label for="node-password">rpcd password <span class="muted">(optional)</span></label>
          <input id="node-password" type="password" value={password} onInput={(e) => setPassword((e.target as HTMLInputElement).value)} autoComplete="current-password" placeholder="Leave blank if the AP has no password" />
        </div>
        <div class="field">
          <label for="node-uplink">Wired uplink interface <span class="muted">(optional)</span></label>
          <input id="node-uplink" value={wiredUplink} onInput={(e) => setWiredUplink((e.target as HTMLInputElement).value)} placeholder="lan1 — leave blank for mesh-only APs" />
        </div>
        {error && <p class="field-error" role="alert">{error}</p>}
      </form>
    </Dialog>
  );
}
