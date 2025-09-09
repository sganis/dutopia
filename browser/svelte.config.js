// Tauri doesn't have a Node.js server to do proper SSR
// so we use adapter-static with a fallback to index.html to put the site in SPA mode
// See: https://svelte.dev/docs/kit/single-page-apps
// See: https://v2.tauri.app/start/frontend/sveltekit/ for more info
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      // write directly into backend app
      pages: '../public',
      assets: '../public',
      fallback: 'index.html',   // SPA fallback
      precompress: false,
      strict: false
    }),
    // leave base empty since we’re serving at /
    paths: { base: '' }
  },
};

export default config;
