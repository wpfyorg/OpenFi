/** Small pill with a colour dot and label — tier names, status words, filter tags. */
export function Chip({ label, color = "var(--text-3)" }: { label: string; color?: string }) {
  return (
    <span class="chip">
      <span class="chip-dot" style={{ background: color }} />
      {label}
    </span>
  );
}
