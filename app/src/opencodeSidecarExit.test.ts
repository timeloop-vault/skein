import { describe, expect, it } from "vitest";
import { createSidecarDeathRecord, sidecarStoppedLine } from "./opencodeSidecarExit.ts";

function record() {
	const writes: string[] = [];
	return { writes, r: createSidecarDeathRecord((s) => writes.push(s)) };
}

describe("createSidecarDeathRecord", () => {
	it("yields nothing when the sidecar did not die", () => {
		const { r, writes } = record();
		expect(r.take()).toBe("");
		expect(writes).toEqual([]);
	});

	it("note then take: the line comes from take, once", () => {
		const { r, writes } = record();
		r.note(3);
		const line = r.take();
		expect(line).toBe(sidecarStoppedLine(3));
		expect(line).toContain("opencode server stopped (exit 3) — use Restart harness to reconnect");
		expect(r.take()).toBe("");
		expect(writes).toEqual([]);
	});

	it("take then note: the line is written by note, once", () => {
		const { r, writes } = record();
		expect(r.take()).toBe("");
		r.note(5);
		expect(writes).toEqual([sidecarStoppedLine(5)]);
	});

	it("prints ? for an unknown code", () => {
		const { r } = record();
		r.note(null);
		expect(r.take()).toContain("(exit ?)");
	});
});
