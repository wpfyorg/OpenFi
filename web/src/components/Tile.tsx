import type { JSX } from "preact";
import { Icon } from "./icons";

/** Coloured icon tile (nav rows, page headers, stat cells). `solid` fills with the colour; otherwise it's a ~15% tint. */
export function Tile({ icon, color, size = 22, solid = false }: { icon: string; color: string; size?: number; solid?: boolean }) {
  const style: JSX.CSSProperties = {
    width: size, height: size, borderRadius: Math.round(size * 0.28),
    background: solid ? color : `color-mix(in srgb, ${color} 15%, transparent)`,
  };
  return (
    <div class="tile" style={style}>
      <Icon name={icon} size={Math.round(size * 0.6)} color={solid ? "#FFFFFF" : color} />
    </div>
  );
}
