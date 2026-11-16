import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { applyAdvancedUci, getAdvancedUci, previewAdvancedUci, type AdvancedUciSections } from "../api";
import { Button } from "../components/Button";
import { ErrorState, Loading } from "../components/States";
import { PageHeader } from "../components/PageHeader";
import { Panel } from "../components/Panel";
import { toast } from "../components/Toast";
import { useSnap } from "../context";

const SECTION = "#3CC4D8";
const CONFIGS = ["system", "wireless", "dhcp"] as const;

/** Identifies the exact editor state — node, config, loaded snapshot, and text — a result used. */
function binding(node: string, config: string, base: AdvancedUciSections | null, text: string) {
  return JSON.stringify([node, config, base, text]);
}

export default function SettingsAdvanced() {
  const { snap } = useSnap();
  const nodes = snap?.nodes ?? [];
  const [node, setNode] = useState("");
  const [config, setConfig] = useState("system");
  const [text, setText] = useState("");
  const [base, setBase] = useState<AdvancedUciSections | null>(null);
  const [revision, setRevision] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A preview is only ever applied to the exact editor state that produced it.
  const [preview, setPreview] = useState<{ binding: string; changes: string[] } | null>(null);
  const loadToken = useRef(0);
  const previewToken = useRef(0);

  useEffect(() => { if (!node && nodes.length) setNode(nodes[0].name); }, [nodes, node]);
  const parsed = useMemo(() => {
    try { return { value: JSON.parse(text) as AdvancedUciSections, error: null }; }
    catch (e) { return { value: null, error: e instanceof Error ? e.message : "Invalid JSON" }; }
  }, [text]);
  const current = useMemo(() => binding(node, config.trim(), base, text), [node, config, base, text]);

  /** Reset the editor and invalidate any in-flight read or preview. */
  function reset() {
    loadToken.current += 1;
    previewToken.current += 1;
    setLoaded(false); setBase(null); setRevision(""); setPreview(null);
  }

  async function load() {
    if (!node || !config.trim()) return;
    const token = ++loadToken.current;
    const targetNode = node;
    const targetConfig = config.trim();
    setBusy(true); setError(null); setPreview(null); setLoaded(false); setBase(null); setRevision("");
    try {
      const result = await getAdvancedUci(targetNode, targetConfig);
      // A newer load or selection owns the editor; this response is stale.
      if (token !== loadToken.current) return;
      setBase(result.sections); setRevision(result.revision); setText(JSON.stringify(result.sections, null, 2)); setLoaded(true);
    } catch (e) {
      if (token !== loadToken.current) return;
      setError(e instanceof Error ? e.message : "Failed to read UCI config");
    }
    finally { if (token === loadToken.current) setBusy(false); }
  }

  async function makePreview() {
    if (!base || !parsed.value) return;
    const token = ++previewToken.current;
    const key = current;
    setBusy(true); setError(null);
    try {
      const result = await previewAdvancedUci(node, config.trim(), revision, base, parsed.value);
      if (token !== previewToken.current) return;
      setPreview({ binding: key, changes: result.changes });
    } catch (e) {
      if (token !== previewToken.current) return;
      setError(e instanceof Error ? e.message : "Validation failed"); setPreview(null);
    }
    finally { if (token === previewToken.current) setBusy(false); }
  }

  async function apply() {
    if (!base || !parsed.value || !preview?.changes.length) return;
    if (preview.binding !== current) {
      setError("The editor changed since this preview; validate again before applying.");
      setPreview(null);
      return;
    }
    setBusy(true); setError(null);
    try {
      const result = await applyAdvancedUci(node, config.trim(), revision, base, parsed.value);
      toast(`Applied ${result.changes.length} UCI change${result.changes.length === 1 ? "" : "s"}`, "success");
      await load();
    } catch (e) { setError(e instanceof Error ? e.message : "UCI apply failed"); }
    finally { setBusy(false); }
  }

  return <div class="page" style={{ "--section": SECTION }}>
    <PageHeader icon="terminal" title="Advanced UCI" subtitle="Edit options for existing UCI sections" />
    <Panel title="Configuration" subtitle="Section names, types, and order are fixed. Credentials are masked; keep the bullet placeholder to preserve the current value.">
      <div class="form-row">
        <label class="field"><span>Node</span><select value={node} disabled={busy} onChange={(e) => { setNode((e.currentTarget as HTMLSelectElement).value); reset(); }}>
          {nodes.map((n) => <option value={n.name}>{n.name}{n.online ? "" : " · offline"}</option>)}
        </select></label>
        <label class="field"><span>UCI config</span><select value={config} disabled={busy} onChange={(e) => { setConfig((e.currentTarget as HTMLSelectElement).value); reset(); }}>
          {CONFIGS.map((name) => <option value={name}>{name}</option>)}
        </select></label>
        <Button primary disabled={!node || !config.trim() || busy} onClick={() => void load()}>{busy && !loaded ? "Loading…" : "Load"}</Button>
      </div>
      {error && <ErrorState message={error} />}
      {loaded && <>
        <label class="field uci-editor-field"><span>Section options · JSON</span><textarea class="uci-editor" spellcheck={false} value={text} onInput={(e) => { setText((e.currentTarget as HTMLTextAreaElement).value); setPreview(null); previewToken.current += 1; }} /></label>
        {parsed.error && <p class="field-error">{parsed.error}</p>}
        <div class="form-actions"><Button disabled={busy || !base || !parsed.value} onClick={() => void makePreview()}>Validate and preview</Button></div>
      </>}
    </Panel>
    {preview && <Panel title="Diff preview" subtitle={preview.changes.length ? `${preview.changes.length} change${preview.changes.length === 1 ? "" : "s"} · secrets are masked` : "No changes"}>
      {preview.changes.length ? <pre class="mono diff-block">{preview.changes.join("\n")}</pre> : <p class="muted">Nothing to apply.</p>}
      <p class="muted small">Apply uses a 30 second router rollback window. OpenFi confirms only after the node answers a fresh health check.</p>
      <Button primary disabled={busy || !preview.changes.length || preview.binding !== current} onClick={() => void apply()}>{busy ? "Applying…" : "Apply with rollback"}</Button>
    </Panel>}
  </div>;
}
