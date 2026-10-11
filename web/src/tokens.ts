// Design tokens mirrored from design/gen_design.py (VARS, NODE, DEVICE, SECTION, NAV).
// Keep these in lock-step with that generator — it is the single visual source of truth.

/** Node identity colour, used everywhere a node is named (tags, mesh rows, topology). */
export const NODE_COLOR: Record<string, string> = {
  mainrouter: "#2CC5C5",
  "ap-2": "#F06BA8",
  "ap-4": "#A48BFF",
};
export const NODE_FALLBACK = "#6B7686";
export function nodeColor(name: string) { return NODE_COLOR[name] ?? NODE_FALLBACK; }

/** Per-device series colours for stacked/grouped charts and share bars; last = "others". */
export const DEVICE_COLORS = ["#7AA2FF", "#FF9F6B", "#E879F9", "#FACC6B", "#6B7686"];
export function deviceColor(i: number) { return DEVICE_COLORS[Math.min(i, DEVICE_COLORS.length - 1)]; }

export const DOWN_COLOR = "#4EA8FF";
export const UP_COLOR = "#F2B14C";

export interface NavItem { icon: string; label: string; hash: string; section: string }

/** Sidebar nav: icon (lucide name), label, hash route, and this page's --section colour. */
export const NAV: NavItem[] = [
  { icon: "gauge", label: "Dashboard", hash: "#/", section: "#4E8BFF" },
  { icon: "network", label: "Topology", hash: "#/topology", section: "#22B8A6" },
  { icon: "arrow-left-right", label: "Traffic", hash: "#/traffic", section: "#2AA7E0" },
  { icon: "chart-column", label: "Usage", hash: "#/usage", section: "#E8A33C" },
  { icon: "monitor-smartphone", label: "Devices", hash: "#/devices", section: "#E0609A" },
  { icon: "wifi", label: "WiFi", hash: "#/wifi", section: "#9476F0" },
  { icon: "shield-check", label: "DNS", hash: "#/dns", section: "#EF5F5F" },
  { icon: "sliders-horizontal", label: "QoS", hash: "#/qos", section: "#F0873C" },
  { icon: "settings", label: "Settings", hash: "#/settings/wifi", section: "#3CC4D8" },
];

/** Section colour for a hash route that may not be in the sidebar (logs, login). */
export function sectionForHash(hash: string): string {
  if (hash.startsWith("#/settings/")) return "#3CC4D8";
  const hit = NAV.find((n) => hash === n.hash || (n.hash !== "#/" && hash.startsWith(n.hash)));
  if (hit) return hit.section;
  if (hash.startsWith("#/logs")) return "#3CC4D8";
  return "#4E8BFF";
}
