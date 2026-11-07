import { useEffect, useMemo, useState } from "preact/hooks";
import { applyAdvancedUci, getAdvancedUci, previewAdvancedUci, type AdvancedUciSections } from "../api";
import { Button } from "../components/Button";
import { ErrorState, Loading } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { toast } from "../components/Toast";
import { useSnap } from "../context";

const SECTION = "#3CC4D8";
const CONFIGS = ["system", "wireless", "dhcp"] as const;

export default function SettingsAdvanced() {
  const { snap } = useSnap();
  const nodes = snap?.nodes ?? [];
  const [node, setNode] = useState("");
  const [config, setConfig] = useState("system");
  const [text, setText] = useState("");
  const [base, setBase] = useState<AdvancedUciSections | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<string[] | null>(null);

  useEffect(() => { if (!node && nodes.length) setNode(nodes[0].name); }, [nodes, node]);
  const parsed = useMemo(() => {
    try { return { value: JSON.parse(text) as AdvancedUciSections, error: null }; }
    catch (e) { return { value: null, error: e instanceof Error ? e.message : "Invalid JSON" }; }
  }, [text]);

  async function load() {
    if (!node || !config.trim()) return;
    setBusy(true); setError(null); setPreview(null); setLoaded(false); setBase(null);
    try {
      const result = await getAdvancedUci(node, config.trim());
      setBase(result.sections); setText(JSON.stringify(result.sections, null, 2)); setLoaded(true);
    } catch (e) { setError(e instanceof Error ? e.message : "Failed to read UCI config"); }
    finally { setBusy(false); }
  }

  async function makePreview() {
    if (!base || !parsed.value) return;
    setBusy(true); setError(null);
    try { setPreview((await previewAdvancedUci(node, config.trim(), base, parsed.value)).changes); }
    catch (e) { setError(e instanceof Error ? e.message : "Validation failed"); setPreview(null); }
    finally { setBusy(false); }
  }

  async function apply() {
    if (!base || !parsed.value || !preview?.length) return;
    setBusy(true); setError(null);
    try {
      const result = await applyAdvancedUci(node, config.trim(), base, parsed.value);
      toast(`Applied ${result.changes.length} UCI change${result.changes.length === 1 ? "" : "s"}`, "success");
      await load();
    } catch (e) { setError(e instanceof Error ? e.message : "UCI apply failed"); }
    finally { setBusy(false); }
  }

  return <div class="page" style={{ "--section": SECTION }}>
    <PageHeader icon="terminal" title="Advanced UCI" subtitle="Edit options for existing UCI sections" />
    <Panel title="Configuration" subtitle="Section names, types, and order are fixed. Credentials are masked; keep the bullet placeholder to preserve the current value.">
      <div class="form-row">
        <label class="field"><span>Node</span><select value={node} onChange={(e) => { setNode((e.currentTarget as HTMLSelectElement).value); setLoaded(false); setBase(null); setPreview(null); }}>
          {nodes.map((n) => <option value={n.name}>{n.name}{n.online ? "" : " · offline"}</option>)}
        </select></label>
        <label class="field"><span>UCI config</span><select value={config} onChange={(e) => { setConfig((e.currentTarget as HTMLSelectElement).value); setLoaded(false); setBase(null); setPreview(null); }}>
          {CONFIGS.map((name) => <option value={name}>{name}</option>)}
        </select></label>
        <Button primary disabled={!node || !config.trim() || busy} onClick={() => void load()}>{busy && !loaded ? "Loading…" : "Load"}</Button>
      </div>
      {error && <ErrorState message={error} />}
      {loaded && <>
        <label class="field uci-editor-field"><span>Section options · JSON</span><textarea class="uci-editor" spellcheck={false} value={text} onInput={(e) => { setText((e.currentTarget as HTMLTextAreaElement).value); setPreview(null); }} /></label>
        {parsed.error && <p class="field-error">{parsed.error}</p>}
        <div class="form-actions"><Button disabled={busy || !base || !parsed.value} onClick={() => void makePreview()}>Validate and preview</Button></div>
      </>}
    </Panel>
    {preview && <Panel title="Diff preview" subtitle={preview.length ? `${preview.length} change${preview.length === 1 ? "" : "s"} · secrets are masked` : "No changes"}>
      {preview.length ? <pre class="mono diff-block">{preview.join("\n")}</pre> : <p class="muted">Nothing to apply.</p>}
      <p class="muted small">Apply uses a 30 second router rollback window. OpenFi confirms only after the node answers a fresh health check.</p>
      <Button primary disabled={busy || !preview.length} onClick={() => void apply()}>{busy ? "Applying…" : "Apply with rollback"}</Button>
    </Panel>}
  </div>;
}
