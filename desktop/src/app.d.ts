// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}

	/** Injected at build time by Vite from package.json. */
	const __APP_VERSION__: string;

	/** Injected at build time: true in the Tauri desktop build, false in the web build. */
	const __DESKTOP__: boolean;
}

export {};
