import { useEffect, useRef, useState } from "preact/hooks";
import { getMe } from "./api";
import type { Snapshot } from "./types";

export type ConnState = "connecting" | "live" | "reconnecting" | "offline";

const MAX_BACKOFF_MS = 15000;

/**
 * Live snapshot over SSE (`/api/v1/stream`), with capped exponential backoff on drop.
 * Only streams while `enabled` (off on the login page); turning it on connects immediately.
 * EventSource can't see status codes, so on a drop it probes `/auth/me`: a 401 means the session
 * is gone (api.ts then sends the user to login) and the stream stops instead of retrying.
 */
export function useSnapshot(enabled = true) {
  const [snap, setSnap] = useState<Snapshot | null>(null);
  const [state, setState] = useState<ConnState>("connecting");
  const attempt = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const es = useRef<EventSource | undefined>(undefined);

  useEffect(() => {
    if (!enabled) return;
    let stopped = false;
    attempt.current = 0;

    function connect() {
      if (stopped) return;
      setState((s) => (s === "live" ? s : attempt.current === 0 ? "connecting" : "reconnecting"));
      const source = new EventSource("/api/v1/stream");
      es.current = source;
      source.addEventListener("snapshot", (e) => {
        attempt.current = 0;
        setState("live");
        try {
          setSnap(JSON.parse((e as MessageEvent).data));
        } catch {
          // ignore a malformed frame, keep the last good snapshot
        }
      });
      source.onerror = async () => {
        source.close();
        if (stopped) return;
        setState("reconnecting");
        try {
          await getMe();
        } catch (e) {
          if ((e as { status?: number }).status === 401) {
            stopped = true;
            setState("offline");
            return;
          }
        }
        if (stopped) return;
        const delay = Math.min(MAX_BACKOFF_MS, 500 * 2 ** attempt.current);
        attempt.current += 1;
        if (attempt.current >= 4) setState("offline");
        timer.current = setTimeout(connect, delay);
      };
    }

    connect();
    return () => {
      stopped = true;
      es.current?.close();
      if (timer.current) clearTimeout(timer.current);
    };
  }, [enabled]);

  return { snap, connected: state === "live", state };
}
