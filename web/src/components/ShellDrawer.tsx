// Root shell over WebSocket (ttyd, proxied through /api/v1/shell/{node}). xterm.js is imported
// lazily — only once this drawer actually opens — so it never lands in the main bundle.
// Flow: step-up password -> POST /shell/token -> open the websocket with the one-time token.
import { useEffect, useRef, useState } from "preact/hooks";
import { getShellToken, ApiError } from "../api";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import { Icon } from "./icons";
// Type-only: erased at compile time, so this does NOT pull xterm into the main bundle.
// The real class is loaded with a runtime `import("@xterm/xterm")` inside openSocket() below.
import type { Terminal as XTerm } from "@xterm/xterm";

const IDLE_MS = 10 * 60 * 1000;
const HARD_CAP_MS = 60 * 60 * 1000;

type Phase = "password" | "connecting" | "connected" | "error";

export function ShellDrawer({ node, onClose }: { node: string; onClose: () => void }) {
  const [phase, setPhase] = useState<Phase>("password");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [idleLeft, setIdleLeft] = useState(IDLE_MS);
  const [capLeft, setCapLeft] = useState(HARD_CAP_MS);
  const termHolder = useRef<HTMLDivElement>(null);
  const ws = useRef<WebSocket | null>(null);
  const termRef = useRef<XTerm | null>(null);
  const idleTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const capTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const tickTimer = useRef<ReturnType<typeof setInterval> | undefined>(undefined);
  const idleDeadline = useRef(0);
  const capDeadline = useRef(0);
  const connectedRef = useRef(false);
  // Server output can arrive before xterm finishes its lazy import + construction; buffer it
  // and flush once the terminal exists so the connection banner and first prompt aren't dropped.
  const pending = useRef<string[]>([]);

  useEffect(() => () => teardown(), []);

  function teardown() {
    clearTimeout(idleTimer.current);
    clearTimeout(capTimer.current);
    clearInterval(tickTimer.current);
    ws.current?.close();
    ws.current = null;
    termRef.current?.dispose();
    termRef.current = null;
    pending.current = [];
  }

  function armIdle() {
    clearTimeout(idleTimer.current);
    idleDeadline.current = Date.now() + IDLE_MS;
    idleTimer.current = setTimeout(() => closeWithReason("Idle for 10 minutes. Session closed."), IDLE_MS);
  }

  function closeWithReason(msg: string) {
    connectedRef.current = false;
    setError(msg);
    setPhase("error");
    teardown();
  }

  async function connect(e: Event) {
    e.preventDefault();
    setError(null);
    setPhase("connecting");
    try {
      const { token } = await getShellToken(password);
      openSocket(token);
    } catch (e) {
      if (e instanceof ApiError && e.status === 403) {
        setError("The browser shell is turned off. Enable it in /etc/openwrtfi.json on the router.");
      } else if (e instanceof ApiError && e.status === 401) {
        setError("Wrong password.");
      } else {
        setError(e instanceof Error ? e.message : "Could not start a shell session.");
      }
      setPhase("password");
    }
  }

  async function openSocket(token: string) {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const url = `${proto}://${location.host}/api/v1/shell/${encodeURIComponent(node)}?token=${encodeURIComponent(token)}`;
    const socket = new WebSocket(url);
    ws.current = socket;

    socket.onopen = async () => {
      const { Terminal } = await import("@xterm/xterm");
      await import("@xterm/xterm/css/xterm.css");
      if (ws.current !== socket) return; // closed while xterm was loading
      const term = new Terminal({
        cols: 100, rows: 28, fontFamily: '"IBM Plex Mono", ui-monospace, monospace', fontSize: 13,
        theme: { background: "#0d1117", foreground: "#e6eaf0" },
        cursorBlink: true,
      });
      if (termHolder.current) term.open(termHolder.current);
      term.onData((data) => { socket.send(data); armIdle(); });
      termRef.current = term;
      if (pending.current.length) { term.write(pending.current.join("")); pending.current = []; }

      capDeadline.current = Date.now() + HARD_CAP_MS;
      capTimer.current = setTimeout(() => closeWithReason("Session reached its 60 minute limit and was closed."), HARD_CAP_MS);
      tickTimer.current = setInterval(() => {
        setIdleLeft(Math.max(0, idleDeadline.current - Date.now()));
        setCapLeft(Math.max(0, capDeadline.current - Date.now()));
      }, 1000);
      armIdle();
      connectedRef.current = true;
      setPhase("connected");
    };
    socket.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        if (termRef.current) termRef.current.write(ev.data);
        else pending.current.push(ev.data);
      }
      armIdle();
    };
    socket.onerror = () => {
      if (!connectedRef.current) closeWithReason("Could not connect to the shell.");
    };
    socket.onclose = (ev) => {
      if (ev.code !== 1000) closeWithReason(connectedRef.current ? "Connection lost." : "Could not connect to the shell.");
    };
  }

  function handleClose() {
    teardown();
    onClose();
  }

  return (
    <Dialog title={`Shell — ${node}`} onClose={handleClose} dialogClass="dialog-wide">
      {phase === "password" && (
        <form class="shell-password-form" onSubmit={connect}>
          <p>Re-enter the root password to open a shell on <b class="mono">{node}</b>.</p>
          <input
            type="password"
            class="text-input"
            value={password}
            onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
            placeholder="Password"
            aria-label="Password"
            autoFocus
          />
          {error && <p class="field-error">{error}</p>}
          <div class="dialog-foot" style={{ padding: 0, border: "none" }}>
            <Button onClick={handleClose}>Cancel</Button>
            <Button type="submit" primary disabled={!password}>Continue</Button>
          </div>
        </form>
      )}
      {phase === "connecting" && <p>Connecting…</p>}
      {phase === "error" && (
        <div class="shell-error">
          <Icon name="triangle-alert" size={20} color="var(--danger)" />
          <p>{error}</p>
          <Button onClick={handleClose}>Close</Button>
        </div>
      )}
      <div class={phase === "connected" ? "shell-body" : "shell-body hidden"}>
        <div class="shell-banner">
          <Icon name="terminal" size={14} />
          <span>Root shell on <b class="mono">{node}</b></span>
          <span class="shell-timers mono small">
            idle {formatMs(idleLeft)} · session {formatMs(capLeft)}
          </span>
        </div>
        <div ref={termHolder} class="shell-terminal" />
      </div>
    </Dialog>
  );
}

function formatMs(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  const m = Math.floor(s / 60);
  const r = s % 60;
  return `${m}:${r.toString().padStart(2, "0")}`;
}
