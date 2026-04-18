// desktop/svelte.config.js
//
// Desktop SvelteKit build. Shares routes / lib / assets with ../browser
// via kit.files so the Svelte source is a single source of truth. The
// only desktop-specific frontend files are this config, vite.config.js,
// tsconfig.json, src/app.html, and the Tauri transport
// (../browser/src/ts/api.tauri.svelte.ts, resolved via the $api alias).

import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      fallback: "index.html",
    }),
    files: {
      routes: "../browser/src/routes",
      lib: "../browser/src/lib",
      assets: "../browser/static",
      appTemplate: "src/app.html",
    },
    alias: {
      $api: "../browser/src/ts/api.tauri.svelte.ts",
    },
  },
};

export default config;
