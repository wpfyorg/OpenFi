// Typed fetch helpers for every /api/v1 route in docs/api.md. Every write sends
// credentials + the X-OpenWrtFi CSRF header; any 401 redirects to the login page.
import type {
  AuditEntry, DailyUsage, DiagRequest, DiagResult, KickRequest, LoginRequest, LoginResponse,
  LogLine, MeResponse, Plan, PlanStatus, RadioPatch, RatePoint, ReserveRequest, Snapshot,
  WhitelistRequest, WifiNetwork, WifiPlanRequest,
} from "./types";

const BASE = "/api/v1";

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

function onUnauthorized() {
  if (location.hash !== "#/login") location.hash = "#/login";
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(BASE + path, {
    credentials: "same-origin",
    ...init,
    headers: { ...(init?.headers ?? {}) },
  });
  if (res.status === 401) {
    onUnauthorized();
    throw new ApiError(401, "unauthorized");
  }
  if (!res.ok) {
    let message = res.statusText;
    try {
      const body = await res.json();
      if (body && typeof body.error === "string") message = body.error;
    } catch {
      // body wasn't JSON — keep statusText
    }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

function write<T>(path: string, method: "POST", body?: unknown): Promise<T> {
  return req<T>(path, {
    method,
    headers: { "Content-Type": "application/json", "X-OpenWrtFi": "1" },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
}

// ---- live data ----
export const getSnapshot = () => req<Snapshot>("/snapshot");

// ---- reads ----
export const getHistory = (range: "24h" | "7d") => req<RatePoint[]>(`/history?range=${range}`);
export const getUsageDaily = (days = 30) => req<DailyUsage[]>(`/usage/daily?days=${days}`);
export const getWifiNetworks = () => req<WifiNetwork[]>("/wifi/networks");
export const getLogs = (opts: { node?: string; lines?: number } = {}) => {
  const p = new URLSearchParams();
  if (opts.node) p.set("node", opts.node);
  p.set("lines", String(opts.lines ?? 200));
  return req<LogLine[]>(`/logs?${p}`);
};
export const getAudit = () => req<AuditEntry[]>("/audit");

// ---- controller nodes ----
export interface ConfiguredNode {
  name: string;
  host: string;
  username: string;
  wired_uplink: string | null;
  gateway: boolean;
  password_set: boolean;
}
export interface AddNodeRequest {
  name: string;
  host: string;
  username: string;
  password: string;
  wired_uplink?: string;
}
export const getConfiguredNodes = () => req<ConfiguredNode[]>("/nodes");
export const addConfiguredNode = (body: AddNodeRequest) => write<ConfiguredNode>("/nodes", "POST", body);
export const removeConfiguredNode = (name: string) =>
  write<{ removed: string }>(`/nodes/${encodeURIComponent(name)}/remove`, "POST");

// ---- diagnostics ----
export const runDiag = (body: DiagRequest) => write<DiagResult>("/diag", "POST", body);

// ---- auth ----
export const login = (body: LoginRequest) => write<LoginResponse>("/auth/login", "POST", body);
export const getMe = () => req<MeResponse>("/auth/me");
export const logout = () => write<void>("/auth/logout", "POST");

// ---- actions ----
export const kickClient = (mac: string, body: KickRequest = {}) =>
  write<void>(`/clients/${encodeURIComponent(mac)}/kick`, "POST", body);
export const reserveClient = (mac: string, body: ReserveRequest) =>
  write<void>(`/clients/${encodeURIComponent(mac)}/reserve`, "POST", body);
export const whitelistClient = (mac: string, body: WhitelistRequest) =>
  write<void>(`/clients/${encodeURIComponent(mac)}/whitelist`, "POST", body);
export const rebootNode = (node: string) => write<void>(`/nodes/${encodeURIComponent(node)}/reboot`, "POST");
export const reloadWifi = (node: string) => write<void>(`/nodes/${encodeURIComponent(node)}/wifi/reload`, "POST");
export const patchRadio = (node: string, radio: string, body: RadioPatch) =>
  write<void>(`/nodes/${encodeURIComponent(node)}/radios/${encodeURIComponent(radio)}`, "POST", body);

// ---- multi-node wifi plan ----
export const planWifiNetwork = (id: string, body: WifiPlanRequest) =>
  write<Plan>(`/wifi/networks/${encodeURIComponent(id)}/plan`, "POST", body);
export const applyWifiPlan = (id: string) => write<Plan>(`/wifi/plans/${encodeURIComponent(id)}/apply`, "POST");
export const getWifiPlanStatus = (id: string) => req<PlanStatus>(`/wifi/plans/${encodeURIComponent(id)}`);

// ---- browser shell step-up ----
// Not yet in docs/api.md's typed section (only prose under "Browser shell"); shaped to match the
// pattern of every other write endpoint. Update docs/api.md + this shape together if the backend differs.
export interface ShellTokenRequest { password: string }
export interface ShellTokenResponse { token: string }
export const getShellToken = (password: string) => write<ShellTokenResponse>("/shell/token", "POST", { password } satisfies ShellTokenRequest);
