import { describe, expect, it } from "vitest";
import { flattenFeed } from "./feedFlatten.ts";
import { firstStoredKind } from "./harnessKind.ts";
import { reducePlan } from "./plan.ts";
import type { HarnessAction } from "./store.ts";

const row = (id: number, over: Partial<HarnessAction>): HarnessAction => ({
	id,
	harnessId: "h1",
	roomId: "r",
	timestampMs: 1000 + id,
	kind: "plan_change",
	payload: "{}",
	source: null,
	...over,
});

const edit = (id: number, harnessKind: string | null): HarnessAction =>
	row(id, {
		kind: "patch",
		harnessKind,
		payload: JSON.stringify({
			tool: "edit",
			files: ["/a/b.ts"],
			patch_info: { additions: 1, deletions: 0 },
		}),
	});

const plan = (id: number, harnessKind: string | null): HarnessAction =>
	row(id, {
		harnessKind,
		payload: JSON.stringify({
			plan_item: { op: "write", items: [{ content: "x", status: "pending" }] },
		}),
	});

describe("firstStoredKind", () => {
	it("takes the first non-null kind", () => {
		expect(firstStoredKind([row(1, {}), row(2, { harnessKind: "claude" })])).toBe("claude");
	});
	it("is null for legacy-only rows", () => {
		expect(firstStoredKind([row(1, { harnessKind: null }), row(2, {})])).toBeNull();
	});
});

describe("kind carried on feed groups", () => {
	it("burst items carry the stored kind", () => {
		const items = flattenFeed([edit(1, null), edit(2, "opencode"), edit(3, "opencode")], {
			showTurnCosts: false,
			liveIds: new Set(),
		});
		const burst = items.find((i) => i.type === "burst");
		expect(burst?.type === "burst" && burst.harnessKind).toBe("opencode");
	});
	it("plan groups carry the stored kind, null for legacy rows", () => {
		expect(reducePlan([plan(1, "claude")])[0]?.harnessKind).toBe("claude");
		expect(reducePlan([plan(1, null)])[0]?.harnessKind).toBeNull();
	});
});
