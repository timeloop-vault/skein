import { describe, expect, it } from "vitest";
import { backgroundEndView } from "./backgroundEnd.ts";

const view = (extra: Record<string, unknown>) =>
	backgroundEndView({ task_id: "t1", task_kind: "bash", ...extra });

describe("backgroundEndView outcome", () => {
	const cases: [string, Record<string, unknown>, string, boolean][] = [
		["completed exit 0", { status: "completed", exit_code: 0 }, "done", false],
		["completed null exit", { status: "completed", exit_code: null }, "done", false],
		["completed nonzero", { status: "completed", exit_code: 2 }, "exit 2", true],
		["failed with exit", { status: "failed", exit_code: 1 }, "failed · exit 1", true],
		["failed no exit", { status: "failed", exit_code: null }, "failed", true],
		["killed", { status: "killed" }, "killed", false],
		["stopped", { status: "stopped" }, "stopped", false],
		["expired", { status: "expired" }, "timed out", false],
		["task_stopped", { status: "task_stopped" }, "stopped by agent", false],
		["unknown", { status: "unknown" }, "ended", false],
		["missing status", {}, "ended", false],
		["novel status", { status: "weird" }, "ended", false],
	];
	for (const [name, payload, outcome, failed] of cases) {
		it(name, () => {
			const v = view(payload);
			expect(v.outcome).toBe(outcome);
			expect(v.failed).toBe(failed);
		});
	}

	it("never says finished", () => {
		for (const [, payload] of cases) expect(view(payload).outcome).not.toMatch(/finished/);
	});
});

describe("backgroundEndView target and kind", () => {
	it("prefers description", () => {
		expect(view({ description: "build", command: "make" }).target).toBe("build");
	});

	it("falls back to the first line of the command", () => {
		expect(view({ description: null, command: "\nnpm test\nmore" }).target).toBe("npm test");
	});

	it("falls back to task_id, then a constant", () => {
		expect(view({}).target).toBe("t1");
		expect(backgroundEndView({}).target).toBe("task");
	});

	it("truncates long targets to 80 chars with an ellipsis", () => {
		const t = view({ command: "x".repeat(200) }).target;
		expect(t).toHaveLength(80);
		expect(t.endsWith("…")).toBe(true);
		expect(view({ command: "y".repeat(80) }).target).toHaveLength(80);
	});

	it("kind falls back to task; duration passes through", () => {
		expect(backgroundEndView({}).kind).toBe("task");
		expect(view({ task_kind: "monitor" }).kind).toBe("monitor");
		expect(view({ duration_ms: 1500 }).durationMs).toBe(1500);
		expect(view({}).durationMs).toBeNull();
	});
});
