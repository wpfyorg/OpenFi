import type { ComponentChildren } from "preact";

/** Standard card surface. Pass `title`/`subtitle`/`right` for the built-in header, or omit for a bare panel. */
export function Panel({
  title, subtitle, right, children, style, class: cls,
}: {
  title?: string; subtitle?: string; right?: ComponentChildren; children: ComponentChildren;
  style?: Record<string, string | number>; class?: string;
}) {
  return (
    <section class={cls ? `panel ${cls}` : "panel"} style={style}>
      {(title || right) && (
        <header class="panel-head">
          <div class="panel-titles">
            <h2>{title}</h2>
            {subtitle && <p>{subtitle}</p>}
          </div>
          {right}
        </header>
      )}
      {children}
    </section>
  );
}
