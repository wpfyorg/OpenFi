import type { ComponentType } from "preact";
import { useEffect, useState } from "preact/hooks";
import { getMe } from "./api";
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
  // No stream on the login page; landing back on the app after sign-in reconnects immediately.
  const { snap, connected, state } = useSnapshot(!isLogin);
  const [mobileNav, setMobileNav] = useState(false);
  const [Page, setPage] = useState<ComponentType | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

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
    getMe().catch(() => {});
  }, []);

  if (isLogin) {
    return (
      <SnapshotContext.Provider value={{ snap, connected, state }}>
        <main class="login-shell">
          {loadError ? <ErrorState message={loadError} /> : Page ? <Page /> : <Loading label="Loading…" />}
        </main>
        <Toaster />
      </SnapshotContext.Provider>
    );
  }

  return (
    <SnapshotContext.Provider value={{ snap, connected, state }}>
      <div class="shell">
        <Sidebar hash={hash} open={mobileNav} onNavigate={() => setMobileNav(false)} />
        <div class="shell-main">
          <Topbar onMenu={() => setMobileNav(true)} />
          <main>
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
