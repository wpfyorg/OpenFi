import type { ComponentChildren, JSX } from "preact";
import { Icon } from "./icons";

/** Button; `primary` fills with the page's --section colour, otherwise it's outlined. */
export function Button({
  children, icon, primary = false, onClick, type = "button", disabled, title,
}: {
  children: ComponentChildren; icon?: string; primary?: boolean; onClick?: JSX.MouseEventHandler<HTMLButtonElement>;
  type?: "button" | "submit"; disabled?: boolean; title?: string;
}) {
  return (
    <button
      type={type}
      class={primary ? "btn btn-primary" : "btn"}
      onClick={onClick}
      disabled={disabled}
      title={title}
    >
      {icon && <Icon name={icon} size={13} />}
      {children}
    </button>
  );
}
