/** Accessible on/off switch. */
export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label?: string; disabled?: boolean }) {
  return (
    <label class={disabled ? "toggle disabled" : "toggle"}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange((e.target as HTMLInputElement).checked)}
      />
      <span class="toggle-track"><span class="toggle-thumb" /></span>
      {label && <span class="toggle-label">{label}</span>}
    </label>
  );
}
