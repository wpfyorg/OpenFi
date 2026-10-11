import type { ComponentType } from "preact";
import { sectionForHash } from "./tokens";

export interface Route {
  hash: string;
  title: string;
  /** Live status subtitle shown under the title while the page has no better one of its own. */
  subtitle: string;
  section: string;
  load: () => Promise<{ default: ComponentType }>;
}

// Single source of truth for navigable pages. Each page is its own file in ./pages, dynamically
// imported so it only loads (and only counts against bundle budget) once its route is visited.
// To add a page: create pages/Foo.tsx exporting `default`, then add one row here.
export const ROUTES: Route[] = [
  { hash: "#/", title: "Dashboard", subtitle: "Network overview", section: sectionForHash("#/"), load: () => import("./pages/Dashboard") },
  { hash: "#/topology", title: "Topology", subtitle: "Mesh map", section: sectionForHash("#/topology"), load: () => import("./pages/Topology") },
  { hash: "#/traffic", title: "Traffic", subtitle: "Live throughput", section: sectionForHash("#/traffic"), load: () => import("./pages/Traffic") },
  { hash: "#/usage", title: "Usage", subtitle: "Monthly accounting", section: sectionForHash("#/usage"), load: () => import("./pages/Usage") },
  { hash: "#/devices", title: "Devices", subtitle: "Every client", section: sectionForHash("#/devices"), load: () => import("./pages/Devices") },
  { hash: "#/wifi", title: "WiFi", subtitle: "Radios and clients", section: sectionForHash("#/wifi"), load: () => import("./pages/Wifi") },
  { hash: "#/dns", title: "DNS filter", subtitle: "AdGuard Home", section: sectionForHash("#/dns"), load: () => import("./pages/Dns") },
  { hash: "#/qos", title: "QoS", subtitle: "Fair use tiers", section: sectionForHash("#/qos"), load: () => import("./pages/Qos") },
  { hash: "#/settings/wifi", title: "Settings", subtitle: "WiFi networks", section: sectionForHash("#/settings/wifi"), load: () => import("./pages/SettingsWifi") },
  { hash: "#/settings/nodes", title: "Settings", subtitle: "Controller nodes", section: sectionForHash("#/settings/nodes"), load: () => import("./pages/SettingsNodes") },
  { hash: "#/logs", title: "Logs", subtitle: "logread, per node", section: sectionForHash("#/logs"), load: () => import("./pages/Logs") },
  { hash: "#/login", title: "Sign in", subtitle: "", section: sectionForHash("#/login"), load: () => import("./pages/Login") },
];

/** Exact match first, then longest-prefix (for nested paths like #/settings/wifi/x); falls back to Dashboard. */
export function matchRoute(hash: string): Route {
  const exact = ROUTES.find((r) => r.hash === hash);
  if (exact) return exact;
  const prefixed = ROUTES
    .filter((r) => r.hash !== "#/" && hash.startsWith(r.hash))
    .sort((a, b) => b.hash.length - a.hash.length)[0];
  return prefixed ?? ROUTES[0];
}
