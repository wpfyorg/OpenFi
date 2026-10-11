import { defineConfig } from "vite";
import preact from "@preact/preset-vite";

// `npm run dev` proxies the API to a local `cargo run -- --mock ../fixtures`.
export default defineConfig({
  plugins: [preact()],
  // ws: true so the browser shell WebSocket (/api/v1/shell/*) is proxied too.
  server: { proxy: { "/api": { target: "http://127.0.0.1:8088", ws: true } } },
  build: { target: "es2020", assetsInlineLimit: 0, reportCompressedSize: true },
});
