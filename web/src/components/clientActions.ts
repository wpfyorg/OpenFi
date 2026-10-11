// Shared write-action wrappers for a client mac: kick / reserve / whitelist, each with a toast on
// success or failure. Used by the Devices client drawer and the Qos per-client whitelist toggle.
// A 401 is already handled globally by api.ts (redirect to login); we still surface a toast so the
// click doesn't look like it silently did nothing before the redirect happens.
import { ApiError, kickClient, reserveClient, whitelistClient } from "../api";
import { toast } from "./Toast";
import type { KickRequest, ReserveRequest } from "../types";

function messageFor(e: unknown, fallback: string): string {
  if (e instanceof ApiError) return e.status === 401 ? "Session expired — signing you out" : e.message || fallback;
  return fallback;
}

export async function kick(mac: string, label: string, body: KickRequest = {}): Promise<boolean> {
  try {
    await kickClient(mac, body);
    toast(`Kicked ${label}`, "success");
    return true;
  } catch (e) {
    toast(messageFor(e, `Couldn't kick ${label}`), "error");
    return false;
  }
}

export async function reserve(mac: string, label: string, body: ReserveRequest): Promise<boolean> {
  try {
    await reserveClient(mac, body);
    toast(`Reserved ${body.ip} for ${label}`, "success");
    return true;
  } catch (e) {
    toast(messageFor(e, `Couldn't reserve an IP for ${label}`), "error");
    return false;
  }
}

/** Whitelist toggle with optimistic UI: caller flips local state immediately, this reverts it on failure. */
export async function whitelist(mac: string, label: string, on: boolean, revert: () => void): Promise<void> {
  try {
    await whitelistClient(mac, { on });
    toast(on ? `Whitelisted ${label}` : `Removed ${label} from the whitelist`, "success");
  } catch (e) {
    revert();
    toast(messageFor(e, `Couldn't update the whitelist for ${label}`), "error");
  }
}
