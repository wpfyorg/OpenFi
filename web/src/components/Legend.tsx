/** Centered row of colour-swatch labels below a chart. */
export function Legend({ items }: { items: { label: string; color: string }[] }) {
  return (
    <div class="legend">
      {items.map((it) => (
        <span class="legend-item">
          <i style={{ background: it.color }} />
          {it.label}
        </span>
      ))}
    </div>
  );
}
