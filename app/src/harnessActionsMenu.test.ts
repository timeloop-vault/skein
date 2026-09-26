import { describe, expect, it } from "vitest";
import { type ActionsButtonState, actionsButtonState } from "./harnessActionsMenu.ts";
import type { NudgeDef } from "./nudgeRegistry.ts";
import type { RepoSkill } from "./repoSkills.ts";

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

const skill = (over: Partial<RepoSkill> = {}): RepoSkill => ({
	command: "deploy",
	name: "deploy",
	description: "Deploy staging.",
	...over,
});

describe("actionsButtonState", () => {
	it("is hidden when the active harness's kind has no terminal", () => {
		const state = actionsButtonState(false, [def()], {}, ok, [], null);
		expect(state).toEqual<ActionsButtonState>({ kind: "hidden" });
	});

	it("is disabled with 'no actions yet' when both the actions scope and skills are empty", () => {
		const state = actionsButtonState(true, [], {}, ok, [], null);
		expect(state).toEqual<ActionsButtonState>({ kind: "disabled", reason: "no actions yet" });
	});

	it("hidden takes priority over an empty list — no terminal means no button at all", () => {
		const state = actionsButtonState(false, [], {}, ok, [], null);
		expect(state).toEqual<ActionsButtonState>({ kind: "hidden" });
	});

	it("builds one menu item per def, resolving the default body with no override", () => {
		const state = actionsButtonState(true, [def()], {}, ok, [], null);
		expect(state).toEqual<ActionsButtonState>({
			kind: "menu",
			items: [
				{
					id: "sweep",
					label: "Actions: sweep",
					title: "a test action",
					body: "Sweep the worktree.",
					gate: { ok: true },
					source: "nudge",
				},
			],
		});
	});

	it("resolves an override's body, but keeps the def's description as the title", () => {
		const state = actionsButtonState(
			true,
			[def()],
			{ sweep: "Custom\nmulti-line body." },
			ok,
			[],
			null,
		);
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items[0]).toMatchObject({
			body: "Custom\nmulti-line body.",
			title: "a test action",
		});
	});

	it("gates each item independently via the supplied gate function", () => {
		const defs = [def(), def({ id: "other", label: "Actions: other", defaultBody: "Other body." })];
		const state = actionsButtonState(
			true,
			defs,
			{},
			(body) => (body === "Other body." ? refused("not safe right now") : ok()),
			[],
			null,
		);
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items.map((i) => i.gate)).toEqual([
			{ ok: true },
			{ ok: false, reason: "not safe right now" },
		]);
	});

	it("is disabled with no skill items when skillInvocation is null, even with skills present", () => {
		const state = actionsButtonState(true, [], {}, ok, [skill()], null);
		expect(state).toEqual<ActionsButtonState>({ kind: "disabled", reason: "no actions yet" });
	});

	it("builds one menu item per skill using the claude-style template", () => {
		const state = actionsButtonState(true, [], {}, ok, [skill()], "/{name}");
		expect(state).toEqual<ActionsButtonState>({
			kind: "menu",
			items: [
				{
					id: "skill:deploy",
					label: "/deploy",
					title: "Deploy staging.",
					body: "/deploy",
					gate: { ok: true },
					source: "skill",
				},
			],
		});
	});

	it("builds skill items using the copilot-style sentence template", () => {
		const state = actionsButtonState(true, [], {}, ok, [skill()], "Use the /{name} skill.");
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items[0]?.body).toBe("Use the /deploy skill.");
	});

	it("falls back the item title to name when description is empty", () => {
		const state = actionsButtonState(
			true,
			[],
			{},
			ok,
			[skill({ description: "", name: "Deploy" })],
			"/{name}",
		);
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items[0]?.title).toBe("Deploy");
	});

	it("orders nudges before skills", () => {
		const state = actionsButtonState(true, [def()], {}, ok, [skill()], "/{name}");
		expect(state.kind).toBe("menu");
		if (state.kind !== "menu") throw new Error("expected menu");
		expect(state.items.map((i) => i.source)).toEqual(["nudge", "skill"]);
	});

	it("is disabled when skills is empty and skillInvocation is non-null but no nudges either", () => {
		const state = actionsButtonState(true, [], {}, ok, [], "/{name}");
		expect(state).toEqual<ActionsButtonState>({ kind: "disabled", reason: "no actions yet" });
	});
});
