/** Segmented control: one of `options` is active; calls `onChange` with the picked value. */
export function Segmented<T extends string>({ options, value, onChange }: { options: T[]; value: T; onChange: (v: T) => void }) {
  return (
    <div class="segmented" role="tablist">
      {options.map((o) => (
        <button
          type="button"
          role="tab"
          aria-selected={o === value}
          class={o === value ? "seg-opt on" : "seg-opt"}
          onClick={() => onChange(o)}
        >
          {o}
        </button>
      ))}
    </div>
  );
}
