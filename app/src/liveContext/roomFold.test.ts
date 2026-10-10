import { describe, expect, it } from "vitest";
import { createRoomFold, deriveRoom, foldRows } from "./roomFold.ts";
import type { HarnessAction } from "./store.ts";

let nextId = 1;
const row = (
	harnessId: string,
	kind: string,
	payload: unknown,
	extra: Partial<HarnessAction> = {},
): HarnessAction => ({
	id: nextId++,
	harnessId,
	roomId: "r",
	timestampMs: 1000 + nextId,
	kind,
	payload: typeof payload === "string" ? payload : JSON.stringify(payload),
	source: null,
	...extra,
});

const plan = (h: string, pi: unknown, extra: Partial<HarnessAction> = {}) =>
	row(h, "plan_change", { plan_item: pi }, extra);
const write = (h: string, items: { content: string; status?: string }[]) =>
	plan(h, { op: "write", items });
const create = (h: string, id: string, subject: string, extra: Partial<HarnessAction> = {}) =>
	plan(h, { op: "create", id, subject, status: "pending" }, extra);
const update = (h: string, id: string, to: string) =>
	plan(h, { op: "update", id, status_change: { from: "pending", to } });

const derive = (rows: HarnessAction[]) => {
	const fold = createRoomFold();
	foldRows(fold, rows);
	return deriveRoom(fold);
};

describe("plan", () => {
	it("latest snapshot wins", () => {
		const g = derive([
			write("h", [{ content: "a", status: "pending" }]),
			write("h", [{ content: "b", status: "completed" }]),
		]).planGroups;
		expect(g).toHaveLength(1);
		expect(g[0]?.items.map((i) => [i.text, i.status])).toEqual([["b", "done"]]);
	});

	it("folds deltas, carrying the subject forward", () => {
		const g = derive([
			create("h", "1", "one"),
			create("h", "2", "two"),
			update("h", "1", "completed"),
		]).planGroups;
		expect(g[0]?.items.map((i) => [i.key, i.text, i.status])).toEqual([
			["h-1", "one", "done"],
			["h-2", "two", "next"],
		]);
	});

	it("a later snapshot ignores earlier deltas, and later deltas too", () => {
		const g = derive([
			create("h", "1", "one"),
			write("h", [{ content: "snap", status: "in_progress" }]),
			create("h", "2", "late"),
		]).planGroups;
		expect(g[0]?.items.map((i) => i.text)).toEqual(["snap"]);
	});

	it("update without create shows by id", () => {
		const g = derive([update("h", "7", "in_progress")]).planGroups;
		expect(g[0]?.items).toEqual([{ key: "h-7", status: "now", text: "7" }]);
	});

	it("keeps first-appearance order and first stored kind, drops empty groups", () => {
		const g = derive([
			create("b", "1", "x", { harnessKind: null }),
			create("a", "1", "y", { harnessKind: "claude" }),
			create("b", "2", "z", { harnessKind: "opencode" }),
			write("empty", []),
		]).planGroups;
		expect(g.map((x) => [x.harnessId, x.harnessKind])).toEqual([
			["b", "opencode"],
			["a", "claude"],
		]);
	});

	it("infers now for a snapshot with a done item and nothing in progress", () => {
		const g = derive([
			write("h", [
				{ content: "a", status: "completed" },
				{ content: "b", status: "pending" },
			]),
		]).planGroups;
		expect(g[0]?.items[1]).toMatchObject({ status: "now", inferred: true });
	});
});

describe("totals", () => {
	const claude = (h: string, rid: string, tokens: number) =>
		row(h, "turn_cost", { request_id: rid, usage: { input_tokens: tokens, output_tokens: 0 } });
	const step = (h: string, cost: number, tokens: number, reason = "stop") =>
		row(h, "turn_cost", { reason, cost, tokens: { total: tokens } });
	const state = (h: string, total: number) => row(h, "cost_state", { total_cost_usd: total });

	it("de-dupes Claude by request_id", () => {
		const t = derive([
			claude("h", "r1", 100),
			claude("h", "r1", 100),
			claude("h", "r2", 50),
		]).totals;
		expect(t).toEqual({ usd: 0, tokens: 150 });
	});

	it("de-dupes opencode steps by harness and payload, counting in-flight ones", () => {
		const s = step("o", 0.5, 10);
		const dup = { ...s, id: nextId++ };
		const flying = step("o", 0.25, 5, "tool-calls");
		const t = derive([s, dup, flying, step("p", 0.5, 10)]).totals;
		expect(t.tokens).toBe(10 + 5 + 10);
		expect(t.usd).toBeCloseTo(0.5 + 0.25 + 0.5);
	});

	it("cost_state supersedes per harness and adds across harnesses", () => {
		const t = derive([state("a", 1), state("a", 2), state("b", 0.5), claude("a", "r", 7)]).totals;
		expect(t).toEqual({ usd: 2.5, tokens: 7 });
	});
});

describe("away summary, count and idle basis", () => {
	it("latest non-empty away_summary wins", () => {
		const d = derive([
			row("h", "away_summary", { content: "first" }),
			row("h", "away_summary", { content: "second" }, { timestampMs: 5000 }),
			row("h", "away_summary", { content: "" }),
			row("h", "away_summary", "not json"),
		]);
		expect(d.awaySummary).toEqual({ content: "second", timestampMs: 5000 });
	});

	it("is null with none", () => {
		expect(derive([row("h", "tool_call", {})]).awaySummary).toBeNull();
	});

	it("counts every row and tracks the max timestamp", () => {
		const d = derive([
			row("h", "tool_call", {}, { timestampMs: 50 }),
			row("h", "tool_call", {}, { timestampMs: 900 }),
			row("h", "tool_call", {}, { timestampMs: 0 }),
		]);
		expect(d.eventCount).toBe(3);
		expect(d.lastActionTs).toBe(900);
		expect(derive([]).lastActionTs).toBe(0);
	});
});

describe("chunked folding", () => {
	it("equals folding everything at once", () => {
		const rows = [
			create("a", "1", "one"),
			row(
				"a",
				"turn_cost",
				{ request_id: "q", usage: { input_tokens: 9 } },
				{ harnessKind: "claude" },
			),
			update("a", "1", "completed"),
			row("o", "turn_cost", { reason: "stop", cost: 0.1, tokens: { total: 3 } }),
			row("a", "cost_state", { total_cost_usd: 1.5 }),
			write("o", [{ content: "x", status: "completed" }, { content: "y" }]),
			row("a", "away_summary", { content: "away" }),
			create("a", "2", "two"),
			row("a", "turn_cost", { request_id: "q", usage: { input_tokens: 9 } }),
		];
		const whole = derive(rows);
		const fold = createRoomFold();
		for (const chunk of [rows.slice(0, 1), rows.slice(1, 4), [], rows.slice(4, 8), rows.slice(8)]) {
			foldRows(fold, chunk);
			deriveRoom(fold);
		}
		expect(deriveRoom(fold)).toEqual(whole);
	});

	it("derive returns fresh objects", () => {
		const fold = createRoomFold();
		foldRows(fold, [create("h", "1", "one"), row("h", "away_summary", { content: "c" })]);
		const a = deriveRoom(fold);
		const b = deriveRoom(fold);
		expect(a.planGroups).not.toBe(b.planGroups);
		expect(a.awaySummary).not.toBe(b.awaySummary);
		expect(a.totals).not.toBe(b.totals);
	});
});
