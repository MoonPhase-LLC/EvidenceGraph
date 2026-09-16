import path from "node:path";
import process from "node:process";
import { randomBytes } from "node:crypto";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(({ command }) => {
  const isDev = command === "serve";

  // --- Development-only Content-Security-Policy -------------------------
  //
  // `app/src-tauri/tauri.conf.json`'s `app.security.csp` (and a `devCsp`
  // override, if one were added) is only injected into HTML that Tauri
  // itself serves, i.e. the production `frontendDist` build delivered over
  // Tauri's own asset protocol. `devUrl` here points at this external Vite
  // dev server instead, so during `tauri dev` the WebView navigates
  // straight to Vite's HTTP response and Tauri never sees or touches it —
  // no CSP reaches the document unless Vite sends one itself as a real
  // response header. `devCsp` alone would not fix this.
  //
  // The nonce is generated once per dev server process (not per request).
  // Vite's `html.cspNonce` stamps this same value onto every script tag it
  // injects into the dev HTML (the HMR client, the React Fast Refresh
  // preamble), so the header below must carry the identical value for
  // those injected scripts to run under `script-src`.
  const devCspNonce = randomBytes(16).toString("base64");

  // Default (non-networked) `tauri dev`: the WebView loads
  // `http://localhost:1420` and Vite's HMR client connects back over a
  // WebSocket to that same origin. `TAURI_DEV_HOST` (mobile/networked dev)
  // moves both the page and HMR to an explicit host/port instead — see the
  // `server.hmr` block below, which this mirrors.
  const devHmrConnectSrc = host ? `ws://${host}:1421` : "ws://localhost:1420";

  // Mirrors the production `csp` in `tauri.conf.json` exactly, plus only
  // the two additions `tauri dev` itself requires:
  //   - `script-src` gets an explicit nonce (instead of relying on
  //     `default-src 'self'`) so the React Fast Refresh preamble Vite
  //     injects as an inline <script> can run, without falling back to
  //     'unsafe-inline' or 'unsafe-eval'.
  //   - `connect-src` gains the HMR WebSocket origin so live reload works.
  const devCsp = [
    "default-src 'self'",
    `script-src 'self' 'nonce-${devCspNonce}'`,
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' asset: http://asset.localhost data:",
    `connect-src ipc: http://ipc.localhost ${devHmrConnectSrc}`,
  ].join("; ");

  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        "@": path.resolve(import.meta.dirname, "./src"),
      },
    },
    // Only stamp a CSP nonce onto Vite's injected script tags in dev; a
    // static nonce baked into the production build would be constant
    // across every load and provide no real protection, and Tauri already
    // injects its own per-load nonces/hashes into the production HTML at
    // compile time.
    ...(isDev ? { html: { cspNonce: devCspNonce } } : {}),

    // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
    //
    // 1. prevent Vite from obscuring rust errors
    clearScreen: false,
    // 2. tauri expects a fixed port, fail if that port is not available
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host
        ? {
            protocol: "ws",
            host,
            port: 1421,
          }
        : undefined,
      headers: isDev
        ? {
            "Content-Security-Policy": devCsp,
          }
        : undefined,
      watch: {
        // 3. tell Vite to ignore watching `src-tauri`
        ignored: ["**/src-tauri/**"],
      },
    },
  };
});
