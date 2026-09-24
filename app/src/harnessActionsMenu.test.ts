import { describe, expect, it } from "vitest";
import { type ActionsButtonState, actionsButtonState, firstLine } from "./harnessActionsMenu.ts";
import type { NudgeDef } from "./nudgeRegistry.ts";

const ok = () => ({ ok: true as const });
const refused = (reason: string) => ({ ok: false as const, reason });

const def = (over: Partial<NudgeDef> = {}): NudgeDef => ({
	id: "sweep",
	label: "Actions: sweep",
	description: "a test action",
	defaultBody: "Sweep the worktree.",
	scope: "actions",
	...over,
});

describe("firstLine", () => {
	it("returns a single-line body unchanged", () => {
		expect(firstLine("one line only")).toBe("one line only");
	});

	it("returns only the text before the first newline", () => {
		expect(firstLine("first\nsecond\nthird")).toBe("first");
	});
});

describe("actionsButtonState", () => {
	it("is hidden when the active harness's kind has no terminal", () => {
		const state = actionsButtonState(false, [def()], {}, ok);
		expect(state).toEqual<ActionsButtonState>({ kind: "hidden" });
	});

	it("is disabled with 'no actions yet' when the actions scope is empty, even with a terminal", () => {
		const state = actionsButtonState(true, [], {}, ok);
		expect(state).toEqual<ActionsButtonState>({ kind: "disabled", reason: "no actions yet" });
	});

	it("hidden takes priority over an empty list — no terminal means no button at all", () => {
		const state = actionsButtonState(false, [], {}, ok);
		expect(state).toEqual<ActionsButtonState>({ kind: "hidden" });
	});

	it("builds one menu item per def, resolving the default body with no override", () => {
		const state = actionsButtonState(true, [def()], {}, ok);
		expect(state).toEqual<ActionsButtonState>({
			kind: "menu",
			items: [
				{
					id: "sweep",
					label: "Actions: sweep",
					title: "Sweep the worktree.",
					body: "Sweep the worktree.",
					gate: { ok: true },
				},
			],
		});
	});

	it("resolves an override's body instead of the default, and its first line as the title", () => {
		const state = actionsButtonState(true, [def()], { sweep: "Custom\nmulti-line body." }, ok);
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items[0]).toMatchObject({
			body: "Custom\nmulti-line body.",
			title: "Custom",
		});
	});

	it("gates each item independently via the supplied gate function", () => {
		const defs = [def(), def({ id: "other", label: "Actions: other", defaultBody: "Other body." })];
		const state = actionsButtonState(true, defs, {}, (body) =>
			body === "Other body." ? refused("not safe right now") : ok(),
		);
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items.map((i) => i.gate)).toEqual([
			{ ok: true },
			{ ok: false, reason: "not safe right now" },
		]);
	});
});
