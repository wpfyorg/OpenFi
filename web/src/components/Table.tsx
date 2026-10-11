import type { ComponentChildren } from "preact";

export interface Column {
  title: string; width?: string | number; align?: "left" | "right";
  /** Optional: makes the header clickable (sortable columns) and shows a direction arrow next to the title. */
  onSort?: () => void; sortDir?: "asc" | "desc";
}

/** Plain data table: header row + body rows, per-column alignment. `rows[i]` length must match `columns`.
 *  `onRowClick`, when given, makes every row a keyboard-focusable button (e.g. to open a detail drawer). */
export function Table({
  columns, rows, empty, onRowClick, rowKey,
}: { columns: Column[]; rows: ComponentChildren[][]; empty?: string; onRowClick?: (i: number) => void; rowKey?: (i: number) => string | number }) {
  return (
    <table class="table">
      <thead>
        <tr>
          {columns.map((c) => (
            <th
              style={{ width: c.width, textAlign: c.align ?? "left", cursor: c.onSort ? "pointer" : undefined }}
              onClick={c.onSort}
            >
              {c.title}
              {c.sortDir && <span class="sort-arrow">{c.sortDir === "asc" ? " ↑" : " ↓"}</span>}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map((r, i) => (
          <tr
            key={rowKey ? rowKey(i) : i}
            class={onRowClick ? "row-click" : undefined}
            tabIndex={onRowClick ? 0 : undefined}
            onClick={onRowClick ? () => onRowClick(i) : undefined}
            onKeyDown={onRowClick ? (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onRowClick(i); } } : undefined}
          >
            {r.map((cell, i2) => (
              <td style={{ textAlign: columns[i2]?.align ?? "left" }}>{cell}</td>
            ))}
          </tr>
        ))}
        {rows.length === 0 && (
          <tr>
            <td colSpan={columns.length} class="muted">{empty ?? "Nothing here yet"}</td>
          </tr>
        )}
      </tbody>
    </table>
  );
}
