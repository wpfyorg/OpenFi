import type { ComponentChildren } from "preact";
import { Icon } from "./icons";

/** Centered placeholder for a panel/page with no data yet. */
export function Empty({ icon = "activity", title, hint }: { icon?: string; title: string; hint?: string }) {
  return (
    <div class="state">
      <Icon name={icon} size={22} color="var(--text-3)" />
      <p class="state-title">{title}</p>
      {hint && <p class="state-hint">{hint}</p>}
    </div>
  );
}

/** Centered loading placeholder — a quiet pulse, respects prefers-reduced-motion via CSS. */
export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <div class="state">
      <span class="spinner" aria-hidden="true" />
      <p class="state-title">{label}</p>
    </div>
  );
}

/** Centered error placeholder with an optional retry action. */
export function ErrorState({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div class="state state-error">
      <Icon name="triangle-alert" size={22} color="var(--danger)" />
      <p class="state-title">Something went wrong</p>
      <p class="state-hint">{message}</p>
      {onRetry && (
        <button type="button" class="btn" onClick={onRetry}>Try again</button>
      )}
    </div>
  );
}

/** Full-page "not built yet" placeholder for routes the next agents will fill in. */
export function ComingSoon({ title, children }: { title: string; children?: ComponentChildren }) {
  return (
    <div class="state state-page">
      <Icon name="activity" size={28} color="var(--section)" />
      <p class="state-title">{title} is coming soon</p>
      {children ?? <p class="state-hint">This page isn't built yet.</p>}
    </div>
  );
}
