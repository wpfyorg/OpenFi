// Diagnostics tool: ping / traceroute / nslookup against POST /diag, with the contract's client-side
// host validation. Used by pages/Logs.tsx.
import { useState } from "preact/hooks";
import { runDiag } from "../api";
import { Button } from "./Button";
import { Empty } from "./States";
import { Panel } from "./Panel";
import { Segmented } from "./Segmented";
import type { DiagResult, DiagTool, Node } from "../types";

const HOST_RE = /^[A-Za-z0-9.:-]{1,253}$/;
const TOOLS: DiagTool[] = ["ping", "traceroute", "nslookup"];

export function DiagPanel({ nodes }: { nodes: Node[] }) {
  const [tool, setTool] = useState<DiagTool>("ping");
  const [host, setHost] = useState("");
  const [node, setNode] = useState<string>("");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<DiagResult | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const trimmed = host.trim();
  const valid = HOST_RE.test(trimmed);
  const touched = host.length > 0;

  async function run(e: Event) {
    e.preventDefault();
    if (!valid) return;
    setRunning(true);
    setErr(null);
    setResult(null);
    try {
      const r = await runDiag({ tool, host: trimmed, node: node || undefined });
      setResult(r);
    } catch (e) {
      setErr(e instanceof Error ? e.message : "Diagnostic failed");
    } finally {
      setRunning(false);
    }
  }

  return (
    <Panel title="Diagnostics" subtitle="Run ping, traceroute or nslookup from a node.">
      <form class="diag-form" onSubmit={run}>
        <Segmented options={TOOLS} value={tool} onChange={setTool} />
        <select class="text-input diag-node-select" value={node} onChange={(e) => setNode((e.target as HTMLSelectElement).value)} aria-label="Run from node">
          <option value="">Gateway (default)</option>
          {nodes.map((n) => <option value={n.name} key={n.name}>{n.name}</option>)}
        </select>
        <input
          class="text-input diag-host-input"
          value={host}
          onInput={(e) => setHost((e.target as HTMLInputElement).value)}
          placeholder="Host or IP, e.g. 1.1.1.1 or example.com"
          aria-label="Host to test"
          aria-invalid={touched && !valid}
        />
        <Button type="submit" primary disabled={!valid || running}>{running ? "Running…" : "Run"}</Button>
      </form>
      {touched && !valid && <p class="field-error">Host must match letters, digits, dots, colons or hyphens only.</p>}
      {err && <p class="field-error">{err}</p>}
      {result ? (
        <>
          <div class="diag-exit"><span class="label">Exit code</span><span class={result.exit_code === 0 ? "mono chip-ok" : "mono chip-bad"}>{result.exit_code}</span></div>
          <pre class="mono diag-output">{result.output || "(no output)"}</pre>
        </>
      ) : !running && (
        <Empty icon="terminal" title="No diagnostic run yet" hint="Pick a tool and a host, then run." />
      )}
    </Panel>
  );
}
