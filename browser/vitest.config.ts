// browser/vitest.config.ts
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  define: {
    __APP_VERSION__: '"test"',
    __DESKTOP__: 'false',
  },
  plugins: [svelte()],
  resolve: {
    // Load browser builds (svelte client runtime, not server/SSR)
    conditions: ['browser'],
    alias: {
      // match svelte.config.js kit.alias used by $api imports
      $api: new URL('./src/ts/api.web.svelte.ts', import.meta.url).pathname,
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    globals: false,
    // Node >= 25 ships its own `localStorage` global, undefined without
    // --localstorage-file. It shadows jsdom's, so store.svelte.ts crashes on
    // import. Turn Node's off and let jsdom provide it.
    execArgv: ['--no-experimental-webstorage'],
  },
});
