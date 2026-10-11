// Real sign-in form (POST /auth/login). On success, returns to the page the user came from —
// app.tsx records the last non-login hash in sessionStorage whenever it changes.
import { useState } from "preact/hooks";
import { ApiError, login } from "../api";
import { Button } from "../components/Button";
import { PageHeader } from "../components/PageHeader";

const RETURN_KEY = "owfi:returnTo";

export default function Login() {
  const [username, setUsername] = useState("root");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: Event) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await login({ username, password });
      let back = "#/";
      try {
        back = sessionStorage.getItem(RETURN_KEY) || "#/";
        sessionStorage.removeItem(RETURN_KEY);
      } catch { /* sessionStorage unavailable (private mode) — fall back to the dashboard */ }
      if (back === "#/login") back = "#/";
      location.hash = back;
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setError("Wrong username or password.");
      else setError(e instanceof Error ? e.message : "Could not sign in.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="page page-center login-page" style={{ "--section": "#4E8BFF" }}>
      <PageHeader icon="log-in" title="Sign in" subtitle="OpenWrtFi control panel" />
      <form class="panel login-form" onSubmit={submit}>
        <div class="field">
          <label for="login-username">Username</label>
          <input
            id="login-username" value={username} onInput={(e) => setUsername((e.target as HTMLInputElement).value)}
            autoComplete="username" autoFocus
          />
        </div>
        <div class="field">
          <label for="login-password">Password <span class="state-hint">(optional)</span></label>
          <input
            id="login-password" type="password" value={password}
            onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
            autoComplete="current-password"
          />
        </div>
        {error && <p class="field-error" role="alert">{error}</p>}
        <Button type="submit" primary disabled={busy || !username}>
          {busy ? "Signing in…" : "Sign in"}
        </Button>
        {import.meta.env.DEV && (
          <p class="state-hint login-hint">Mock mode accepts username "root" and password "mock".</p>
        )}
      </form>
    </div>
  );
}
