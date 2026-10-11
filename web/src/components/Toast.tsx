import { useEffect, useState } from "preact/hooks";

export type ToastKind = "info" | "success" | "error";
interface ToastMsg { id: number; kind: ToastKind; text: string }

let seq = 0;
let items: ToastMsg[] = [];
const subs = new Set<(items: ToastMsg[]) => void>();
function emit() { subs.forEach((fn) => fn(items)); }

/** Fire a toast from anywhere (no context needed). Render <Toaster/> once near the app root. */
export function toast(text: string, kind: ToastKind = "info", ms = 4000) {
  const id = ++seq;
  items = [...items, { id, kind, text }];
  emit();
  setTimeout(() => { items = items.filter((t) => t.id !== id); emit(); }, ms);
}

/** Renders queued toast() calls, bottom-right, stacked. */
export function Toaster() {
  const [list, setList] = useState<ToastMsg[]>(items);
  useEffect(() => {
    subs.add(setList);
    return () => { subs.delete(setList); };
  }, []);
  if (list.length === 0) return null;
  return (
    <div class="toast-stack" role="status" aria-live="polite">
      {list.map((t) => (
        <div class={`toast toast-${t.kind}`}>{t.text}</div>
      ))}
    </div>
  );
}
