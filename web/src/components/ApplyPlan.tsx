// Renders a wifi multi-node change: the masked per-node diff, the rollout order, and (once applying)
// a live per-node timeline with a 30s rollback countdown. Purely presentational — pages/SettingsWifi.tsx
// owns the plan/apply/poll lifecycle and passes the current Plan + PlanStatus down.
import { useEffect, useState } from "preact/hooks";
import { Icon } from "./icons";
import { NodeTag } from "./NodeTag";
import type { Plan, PlanStepStatus, PlanStatus } from "../types";

export interface ApplyPlanProps {
  plan: Plan;
  status: PlanStatus | null;
}

export function ApplyPlan({ plan, status }: ApplyPlanProps) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (!status?.steps.some((s) => s.status === "confirming")) return;
    const id = setInterval(() => setNow(Date.now() / 1000), 250);
    return () => clearInterval(id);
  }, [status]);

  return (
    <div class="apply-plan">
      <div class="apply-plan-diff">
        <span class="label">Pending changes</span>
        <pre class="mono diff-block">
          {plan.diff.flatMap((d) => d.lines.map((line) => `${d.node.padEnd(12)} ${line}`)).join("\n") || "No changes"}
        </pre>
      </div>
      <div class="apply-plan-timeline">
        <span class="label">Apply plan · applies to {plan.order.length} node{plan.order.length === 1 ? "" : "s"}, in order</span>
        {plan.order.map((node) => {
          const step = status?.steps.find((s) => s.node === node);
          return <StepRow key={node} node={node} status={step?.status} deadline={step?.deadline ?? null} error={step?.error ?? null} now={now} />;
        })}
      </div>
    </div>
  );
}

function StepRow({
  node, status, deadline, error, now,
}: { node: string; status?: PlanStepStatus; deadline: number | null; error: string | null; now: number }) {
  const info = STEP_INFO[status ?? "queued"];
  const countdown = status === "confirming" && deadline != null ? Math.max(0, Math.round(deadline - now)) : null;
  return (
    <div class="plan-step-row">
      <Icon name={info.icon} size={14} color={info.color} class={status === "applying" ? "spin" : undefined} />
      <NodeTag node={node} size={12} />
      <span class="mono small plan-step-status" style={{ color: info.color }}>
        {info.label}
        {countdown != null ? ` · rollback ${countdown}s` : ""}
      </span>
      {error && <span class="mono small muted plan-step-error">{error}</span>}
    </div>
  );
}

const STEP_INFO: Record<PlanStepStatus, { icon: string; label: string; color: string }> = {
  queued: { icon: "circle-dot", label: "queued", color: "var(--text-3)" },
  applying: { icon: "loader-circle", label: "applying", color: "var(--section)" },
  confirming: { icon: "clock", label: "confirming", color: "var(--warn)" },
  confirmed: { icon: "circle-check", label: "confirmed", color: "var(--ok)" },
  rolled_back: { icon: "rotate-ccw", label: "rolled back", color: "var(--danger)" },
  failed: { icon: "circle-x", label: "failed", color: "var(--danger)" },
  skipped: { icon: "ban", label: "skipped", color: "var(--text-3)" },
};
