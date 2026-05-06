import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// @ts-expect-error — process is a Node global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
	plugins: [react()],

	// Use terser instead of Vite's default esbuild minifier.
	//
	// esbuild's minify pass breaks xterm.js v6's parser in bundled
	// builds. Symptom: opencode's opentui-based TUI emits its
	// capability-query burst on startup, xterm.js fires `onData` for
	// OSC 10 / OSC 11 / first CPR (3 replies), then silently stops
	// emitting replies for the rest of the chunk — including the
	// DECRQM mode replies opentui needs. opentui draws one
	// alt-screen frame of background fill, then exits with code 0.
	// Reproduces deterministically with `minify: "esbuild"`,
	// disappears with `"terser"` or `false`. Same xterm.js bundle on
	// disk in all three; the trigger is the way esbuild rewrites the
	// surrounding chunk.
	//
	// Cost vs esbuild: build is ~5–10 s slower, bundle is ~5–10 %
	// larger. No upstream xterm.js issue matches our exact symptom
	// — worth filing once we have a minimal repro.
	build: {
		minify: "terser",
	},

	// Vite options tailored for Tauri development.
	clearScreen: false,
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
		watch: {
			ignored: ["**/src-tauri/**"],
		},
	},
}));
