// desktop/vite.config.js
import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const pkg = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
);
const browserPkg = JSON.parse(
  readFileSync(new URL("../browser/package.json", import.meta.url), "utf8"),
);

// Allow `serve` to reach source in both `desktop/` and `../browser/`.
const browserDir = fileURLToPath(new URL("../browser", import.meta.url));

const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [sveltekit(), tailwindcss()],

  // Vite must serve the parent dir so SvelteKit can reach ../browser/src.
  server: {
    fs: { allow: [".", browserDir] },
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: "ws", host, port: 1421 }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },

  clearScreen: false,

  define: {
    // Shared with browser build. True here so the desktop-only code paths
    // (ScanPanel, reveal/terminal/delete actions, auth bypass) compile in.
    __DESKTOP__: "true",
    __APP_VERSION__: JSON.stringify(browserPkg.version ?? pkg.version),
  },

  build: { sourcemap: true },
}));
