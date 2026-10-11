import { createContext } from "preact";
import { useContext } from "preact/hooks";
import type { Snapshot } from "./types";
import type { ConnState } from "./useSnapshot";

export interface SnapshotCtx { snap: Snapshot | null; state: ConnState; connected: boolean }

/** The single live Snapshot + connection state, provided once at the app root by App.tsx. */
export const SnapshotContext = createContext<SnapshotCtx>({ snap: null, state: "connecting", connected: false });

/** Read the live Snapshot from anywhere without prop drilling; re-renders on every poll. */
export const useSnap = () => useContext(SnapshotContext);
