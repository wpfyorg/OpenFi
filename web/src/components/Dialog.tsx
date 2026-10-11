import type { ComponentChildren } from "preact";
import { useEffect, useRef } from "preact/hooks";
import { Button } from "./Button";
import { Icon } from "./icons";

/** Focus-trapped modal. Esc and the backdrop close it; focus returns to the opener on unmount. */
export function Dialog({
  title, onClose, children, footer, dialogClass,
}: { title: string; onClose: () => void; children: ComponentChildren; footer?: ComponentChildren; dialogClass?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const opener = useRef<Element | null>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    opener.current = document.activeElement;
    const node = ref.current;
    const focusable = () => node?.querySelectorAll<HTMLElement>('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])');
    // Prefer a form control marked autofocus over the header's close button. More importantly,
    // only run this focus setup once: pages with live snapshot data re-render every few seconds,
    // and a changing inline onClose callback must not steal focus from an input mid-typing.
    (node?.querySelector<HTMLElement>('[autofocus]') ?? focusable()?.[0])?.focus();

    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") { closeRef.current(); return; }
      if (e.key !== "Tab") return;
      const items = Array.from(focusable() ?? []);
      if (items.length === 0) return;
      const first = items[0], last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      (opener.current as HTMLElement | null)?.focus?.();
    };
  }, []);

  return (
    <div class="dialog-backdrop" onClick={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div class={dialogClass ? `dialog ${dialogClass}` : "dialog"} role="dialog" aria-modal="true" aria-labelledby="dialog-title" ref={ref}>
        <header class="dialog-head">
          <h2 id="dialog-title">{title}</h2>
          <button type="button" class="icon-btn" onClick={onClose} aria-label="Close">
            <Icon name="x" size={16} />
          </button>
        </header>
        <div class="dialog-body">{children}</div>
        {footer && <footer class="dialog-foot">{footer}</footer>}
      </div>
    </div>
  );
}

/** Dialog pre-wired with Cancel / confirm actions for a destructive or write action. */
export function ConfirmDialog({
  title, body, confirmLabel = "Confirm", danger = false, onConfirm, onClose,
}: { title: string; body: ComponentChildren; confirmLabel?: string; danger?: boolean; onConfirm: () => void; onClose: () => void }) {
  return (
    <Dialog
      title={title}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <button type="button" class={danger ? "btn btn-danger" : "btn btn-primary"} onClick={() => { onConfirm(); onClose(); }}>
            {confirmLabel}
          </button>
        </>
      }
    >
      {body}
    </Dialog>
  );
}
