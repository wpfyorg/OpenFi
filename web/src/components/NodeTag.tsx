import { nodeColor } from "../tokens";

/** A node's name in its identity colour, with a small swatch — used anywhere a node is referenced. */
export function NodeTag({ node, size = 12 }: { node: string; size?: number }) {
  return (
    <span class="node-tag" style={{ fontSize: size }}>
      <span class="node-tag-sw" style={{ background: nodeColor(node), height: size }} />
      <span class="mono">{node}</span>
    </span>
  );
}
