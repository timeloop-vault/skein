import { describe, expect, it, vi } from "vitest";
import { ArgvPrepareError } from "./argvFailure.ts";
import { runUserCreation } from "./creationOutcome.ts";

describe("runUserCreation", () => {
	it("reports success without calling report", async () => {
		const report = vi.fn();
		expect(await runUserCreation(async () => 1, report)).toEqual({ ok: true });
		expect(report).not.toHaveBeenCalled();
	});

	it("turns an argv failure into a reported failure result", async () => {
		const report = vi.fn();
		const out = await runUserCreation(async () => {
			throw new ArgvPrepareError("boom");
		}, report);
		expect(out.ok).toBe(false);
		if (!out.ok) expect(out.error).toContain("boom");
		expect(report).toHaveBeenCalledTimes(1);
	});

	it("rethrows anything that is not an argv failure", async () => {
		const report = vi.fn();
		await expect(
			runUserCreation(async () => {
				throw new Error("bug");
			}, report),
		).rejects.toThrow("bug");
		expect(report).not.toHaveBeenCalled();
	});
});
