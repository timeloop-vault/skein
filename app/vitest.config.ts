import { defineConfig } from "vitest/config";

// Frontend test infra (#169). Deliberately minimal and deliberately
// separate from vite.config.ts: these suites cover pure modules — argv
// construction, reducers, payload parsing — so they need neither the
// React plugin nor a DOM. A component suite later can add jsdom via a
// per-file `@vitest-environment` pragma without slowing this down.
//
// Not wired into `npm run build`; the pre-commit hook and CI run it as
// its own step (`npm test`).
export default defineConfig({
	test: {
		environment: "node",
		include: ["src/**/*.test.ts"],
	},
});
