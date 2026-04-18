import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import pkg from './package.json' with { type: 'json' };

export default defineConfig({
	plugins: [sveltekit(), tailwindcss()],
	build: { sourcemap: true },
	define: {
		__APP_VERSION__: JSON.stringify(pkg.version),
	},
});
