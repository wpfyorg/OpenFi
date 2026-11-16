import type { ComponentType } from "preact";
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { getMe } from "./api";
import type { MeResponse } from "./types";
import { Sidebar } from "./components/Sidebar";
import { Loading, ErrorState } from "./components/States";
import { Toaster } from "./components/Toast";
import { Topbar } from "./components/Topbar";
import { SnapshotContext } from "./context";
import { useHash } from "./router";
import { matchRoute } from "./routes";
import { useSnapshot } from "./useSnapshot";

// Login.tsx reads this back after a successful sign-in to return to where the user came from.
const RETURN_KEY = "owfi:returnTo";

export function App() {
  const hash = useHash();
  const route = matchRoute(hash);
  const isLogin = hash === "#/login";
  const [mobileNav, setMobileNav] = useState(false);
  const [Page, setPage] = useState<ComponentType | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [permissions, setPermissions] = useState<Record<string, import("./types").NodePermissions>>({});
  // Kept apart from the permission map: an empty map is a real answer (nothing granted), while
  // this flag means the probe failed and nothing is known yet.
  const [permissionError, setPermissionError] = useState(false);
  const permissionAttempt = useRef(0);
  const permissionTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const applyMe = useCallback((me: MeResponse) => {
    setPermissions(me.permissions ?? {});
    setPermissionError(false);
    permissionAttempt.current = 0;
  }, []);
  // No stream on the login page; landing back on the app after sign-in reconnects immediately.
  const { snap, connected, state } = useSnapshot(!isLogin, applyMe);

  useEffect(() => {
    let cancelled = false;
    setPage(null);
    setLoadError(null);
    route.load().then(
      (m) => { if (!cancelled) setPage(() => m.default); },
      (err) => { if (!cancelled) setLoadError(String(err)); },
    );
    return () => { cancelled = true; };
  }, [route.hash]);

  useEffect(() => setMobileNav(false), [hash]);

  // Remember the last non-login page so Login.tsx can send the user back after signing in.
  useEffect(() => {
    if (isLogin) return;
    try { sessionStorage.setItem(RETURN_KEY, hash); } catch { /* private mode: skip, worst case land on "#/" */ }
  }, [hash, isLogin]);

  // Light auth guard: probe once on load. api.ts's req() already redirects to #/login on any 401,
  // so this only needs to fire the request — while the backend has no auth yet, /auth/me 404s and
  // is swallowed here, leaving the app usable so mock/dev work isn't blocked on login.
  useEffect(() => {
    if (isLogin) return;
    let cancelled = false;
    const load = () => {
      getMe().then(
        (me) => { if (!cancelled) applyMe(me); },
        (e) => {
          if (cancelled) return;
          const status = (e as { status?: number }).status;
          if (status === 401) return; // api.ts already sent the user to login
          setPermissionError(true);
          // Only transient failures retry (offline, 5xx such as a 502 from a booting daemon),
          // and only a bounded number of times; a reconnect probe still applies permissions.
          const transient = status === undefined || status >= 500;
          if (!transient || permissionAttempt.current >= 4) return;
          const delay = Math.min(15000, 500 * 2 ** permissionAttempt.current);
          permissionAttempt.current += 1;
          permissionTimer.current = setTimeout(load, delay);
        },
      );
    };
    load();
    return () => {
      cancelled = true;
      if (permissionTimer.current) clearTimeout(permissionTimer.current);
    };
  }, [isLogin, applyMe]);

  if (isLogin) {
    return (
      <SnapshotContext.Provider value={{ snap, connected, state, permissions }}>
        <main class="login-shell">
          {loadError ? <ErrorState message={loadError} /> : Page ? <Page /> : <Loading label="Loading…" />}
        </main>
        <Toaster />
      </SnapshotContext.Provider>
    );
  }

  return (
    <SnapshotContext.Provider value={{ snap, connected, state, permissions }}>
      <div class="shell">
        <Sidebar hash={hash} open={mobileNav} onNavigate={() => setMobileNav(false)} />
        <div class="shell-main">
          <Topbar onMenu={() => setMobileNav(true)} />
          <main>
            {permissionError && (
              <p class="muted small" role="status">Could not load node permissions; node actions may be unavailable until the next successful refresh.</p>
            )}
            {loadError ? (
              <ErrorState message={loadError} />
            ) : Page ? (
              <Page />
            ) : (
              <Loading label={`Loading ${route.title}…`} />
            )}
          </main>
        </div>
      </div>
      <Toaster />
    </SnapshotContext.Provider>
  );
}
