import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The dev server proxies API calls to the Rust server so the client can use
// same-origin relative URLs in both dev and production.
const API_TARGET = process.env.JF_API ?? "http://localhost:8096";

const PROXY_PREFIXES = [
  "/Users",
  "/Sessions",
  "/System",
  "/Items",
  "/Shows",
  "/Videos",
  "/Plugins",
  "/Events",
  "/UserPlayedItems",
  "/UserFavoriteItems",
];

export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    // Bind all interfaces so the client is reachable from another device on
    // the LAN, which is the common way to use a media server.
    host: true,
    proxy: Object.fromEntries(
      PROXY_PREFIXES.map((prefix) => [
        prefix,
        { target: API_TARGET, changeOrigin: true, ws: true },
      ]),
    ),
  },
  build: {
    outDir: "dist",
    sourcemap: true,
    // The WASM-free client is small; a single bundle beats many requests on a
    // home network.
    chunkSizeWarningLimit: 900,
  },
});