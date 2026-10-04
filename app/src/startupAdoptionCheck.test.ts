import { afterEach, describe, expect, it, vi } from "vitest";
import { startupAdoption } from "./startupAdoption.ts";
import {
	type AdoptionDeps,
	type AdoptionHarness,
	runAdoptionCheck,
} from "./startupAdoptionCheck.ts";

const H = "h1";
const claude = (sessionId = "A"): AdoptionHarness => ({ kind: "claude", sessionId, cwd: "/w" });

function deps(over: Partial<AdoptionDeps> & { present?: string[] } = {}) {
	const present = over.present ?? [];
	const adopt = vi.fn();
	const d: AdoptionDeps = {
		lookup: () => claude(),
		isShell: () => false,
		exists: async (id) => present.includes(id),
		adopt,
		...over,
	};
	return { d, adopt };
}

afterEach(() => startupAdoption.forget(H));

describe("runAdoptionCheck", () => {
	it("adopts when bound is missing and candidate exists", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({ present: ["B"] });
		await runAdoptionCheck(d);
		expect(adopt).toHaveBeenCalledWith("r", H, "B");
		expect(startupAdoption.pending(H)).toBeNull();
	});

	it("drops when bound exists", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({ present: ["A", "B"] });
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		expect(startupAdoption.pending(H)).toBeNull();
	});

	it("waits when neither exists", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps();
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		expect(startupAdoption.pending(H)).not.toBeNull();
	});

	it("forgets when the stored id moved", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({ present: ["B"], lookup: () => claude("Z") });
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		expect(startupAdoption.pending(H)).toBeNull();
	});

	it("forgets in shell mode", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({ present: ["B"], isShell: () => true });
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		expect(startupAdoption.pending(H)).toBeNull();
	});

	it("does not adopt when the bound stat errors", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({
			exists: async (id) => {
				if (id === "A") throw new Error("boom");
				return true;
			},
		});
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
	});

	it("times out a hung bound stat, adopts nothing, and does not wedge later runs", async () => {
		startupAdoption.note(H, "r", "A", "B");
		const { d, adopt } = deps({ exists: () => new Promise<boolean>(() => {}), timeoutMs: 10 });
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		const next = deps({ present: ["B"], timeoutMs: 10 });
		startupAdoption.note(H, "r", "A", "B");
		await runAdoptionCheck(next.d);
		expect(next.adopt).toHaveBeenCalledWith("r", H, "B");
	});

	it("does not adopt when the id moved during the await", async () => {
		startupAdoption.note(H, "r", "A", "B");
		let current = "A";
		const { d, adopt } = deps({
			lookup: () => claude(current),
			exists: async (id) => {
				current = "Z";
				return id === "B";
			},
		});
		await runAdoptionCheck(d);
		expect(adopt).not.toHaveBeenCalled();
		expect(startupAdoption.pending(H)).toBeNull();
	});
});
