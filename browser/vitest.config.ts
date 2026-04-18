// browser/vitest.config.ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  define: {
    __APP_VERSION__: '"test"',
    __DESKTOP__: 'false',
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    globals: false,
  },
});
