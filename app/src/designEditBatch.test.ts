import { describe, expect, it } from "vitest";
import { applyChange, batchSize, batchToProposal, emptyBatch } from "./designEditBatch.ts";
import type { Change } from "./designProposal.ts";

const style = (property: string, from: string, to: string, token?: string): Change =>
	token ? { kind: "style", property, from, to, token } : { kind: "style", property, from, to };

const fold = (...cs: Change[]) => cs.reduce(applyChange, emptyBatch());

describe("edit batch", () => {
	it("is empty at first", () => {
		expect(batchSize(emptyBatch())).toBe(0);
		expect(batchToProposal(emptyBatch())).toBeNull();
	});

	it("keeps the first from and the latest to and token", () => {
		const b = fold(
			style("padding-left", "12px", "14px"),
			style("padding-left", "14px", "16px", "--space-4"),
		);
		expect(batchToProposal(b)).toEqual({
			changes: [style("padding-left", "12px", "16px", "--space-4")],
		});
		const c = applyChange(b, style("padding-left", "16px", "18px"));
		expect(batchToProposal(c)?.changes).toEqual([style("padding-left", "12px", "18px")]);
	});

	it("drops a style change that returns to its original", () => {
		const b = fold(style("color", "red", "blue"), style("color", "blue", "red"));
		expect(batchSize(b)).toBe(0);
		// and a later edit starts from the original again
		expect(batchToProposal(applyChange(b, style("color", "red", "green")))?.changes).toEqual([
			style("color", "red", "green"),
		]);
	});

	it("replaces the cumulative offset and removes it at 0,0", () => {
		const b = fold({ kind: "offset", dx: 1, dy: 0 }, { kind: "offset", dx: 4, dy: 2 });
		expect(b.offset).toEqual({ kind: "offset", dx: 4, dy: 2 });
		expect(batchSize(applyChange(b, { kind: "offset", dx: 0, dy: 0 }))).toBe(0);
	});

	it("merges text and drops it on revert", () => {
		const b = fold(
			{ kind: "text", from: "Save", to: "Sav" },
			{ kind: "text", from: "Sav", to: "Go" },
		);
		expect(b.text).toEqual({ kind: "text", from: "Save", to: "Go" });
		expect(batchSize(applyChange(b, { kind: "text", from: "Go", to: "Save" }))).toBe(0);
	});

	it("orders style by allowlist, then offset, then text", () => {
		const b = fold(
			{ kind: "text", from: "a", to: "b" },
			{ kind: "offset", dx: 1, dy: 1 },
			style("opacity", "1", "0.5"),
			style("width", "10px", "20px"),
		);
		expect(
			batchToProposal(b)?.changes.map((c) => (c.kind === "style" ? c.property : c.kind)),
		).toEqual(["width", "opacity", "offset", "text"]);
		expect(batchSize(b)).toBe(4);
	});

	it("does not mutate the previous batch", () => {
		const a = fold(style("color", "red", "blue"));
		applyChange(a, style("color", "blue", "red"));
		expect(batchSize(a)).toBe(1);
	});
});
