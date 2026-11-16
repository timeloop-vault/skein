import { describe, expect, it } from "vitest";
import { buildFilesDirtyAnswer } from "./agentRequestsFiles.ts";

const lookup = (m: Record<string, string[]>) => (id: string) => m[id] ?? [];

describe("files.dirty", () => {
	it("answers empty when all clean", () => {
		const r = buildFilesDirtyAnswer({ harnessIds: ["a", "b"] }, lookup({}));
		expect(r).toEqual({ ok: true, value: { dirty: [] } });
	});

	it("lists only the dirty one of several", () => {
		const r = buildFilesDirtyAnswer({ harnessIds: ["a", "b", "c"] }, lookup({ b: ["x.ts"] }));
		expect(r).toEqual({ ok: true, value: { dirty: [{ harnessId: "b", names: ["x.ts"] }] } });
	});

	it("keeps the order asked", () => {
		const r = buildFilesDirtyAnswer(
			{ harnessIds: ["c", "a"] },
			lookup({ a: ["1"], c: ["2", "3"] }),
		);
		expect(r).toEqual({
			ok: true,
			value: {
				dirty: [
					{ harnessId: "c", names: ["2", "3"] },
					{ harnessId: "a", names: ["1"] },
				],
			},
		});
	});

	it("handles an empty id list", () => {
		expect(buildFilesDirtyAnswer({ harnessIds: [] }, lookup({ a: ["x"] }))).toEqual({
			ok: true,
			value: { dirty: [] },
		});
	});

	it("rejects malformed args", () => {
		for (const bad of [null, "x", [], {}, { harnessIds: "a" }, { harnessIds: ["a", 1] }]) {
			const r = buildFilesDirtyAnswer(bad, lookup({}));
			expect(r.ok).toBe(false);
		}
	});
});
