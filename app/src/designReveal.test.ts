import { describe, expect, it, vi } from "vitest";
import { handleDesignRequest } from "./designAgentRequests.ts";
import { type DesignPaneApi, parseShowElementArgs, registerDesignPane } from "./designControl.ts";
import {
	afterReveal,
	type InvokedBeacon,
	invokeAnchorResult,
	invokeSelectorResult,
	NOT_VISIBLE_ERROR,
	parseInvokeElementArgs,
	REVEAL_SKIPPED_ERROR,
} from "./designInvoke.ts";
import { pollUntil, revealDecision, revealPane } from "./designReveal.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";

const el: ElementDescriptor = {
	selector: "p",
	tag: "p",
	text: "hi",
	attrs: {},
	rect: { x: 1, y: 2, w: 3, h: 4 },
};
const base = { roomId: "r", harnessId: "h" };
const beacon = (b: Partial<InvokedBeacon>): InvokedBeacon => ({
	type: "invoked",
	requestId: "agent-inv-1",
	count: 1,
	element: el,
	...b,
});

describe("reveal parsing", () => {
	it("carries only true, rejects non-booleans, for both verbs", () => {
		const s = { ...base, selector: "p" };
		const i = { ...s, action: "tap" };
		for (const parse of [parseShowElementArgs, parseInvokeElementArgs]) {
			const args = parse === parseShowElementArgs ? s : i;
			const on = parse({ ...args, reveal: true });
			expect(on.ok && on.value.reveal).toBe(true);
			const off = parse({ ...args, reveal: false });
			expect(off.ok && "reveal" in off.value).toBe(false);
			const none = parse({ ...args, reveal: null });
			expect(none.ok && "reveal" in none.value).toBe(false);
			for (const bad of ["yes", 1, {}]) {
				const r = parse({ ...args, reveal: bad });
				expect(r.ok).toBe(false);
				expect(!r.ok && r.error).toMatch(/^bad_arguments: reveal/);
			}
		}
	});
});

describe("afterReveal", () => {
	it("rewrites not_visible only when reveal was requested and skipped", () => {
		const nv = new Error(NOT_VISIBLE_ERROR);
		expect((afterReveal(nv, false) as Error).message).toBe(REVEAL_SKIPPED_ERROR);
		expect(afterReveal(nv, true)).toBe(nv);
		expect(afterReveal(nv, undefined)).toBe(nv);
		const other = new Error("busy: x");
		expect(afterReveal(other, false)).toBe(other);
	});
});

describe("invoke answer shaping", () => {
	it("maps notVisible to the exact error", () => {
		expect(() => invokeSelectorResult(beacon({ notVisible: true }), "swipe")).toThrow(
			NOT_VISIBLE_ERROR,
		);
		expect(() =>
			invokeAnchorResult(
				{ tier: "anchored", highlighted: true, element: el },
				beacon({ notVisible: true }),
				"swipe",
			),
		).toThrow(NOT_VISIBLE_ERROR);
		expect(NOT_VISIBLE_ERROR).toMatch(/^not_visible: the target has no layout/);
	});
	it("passes visible through", () => {
		expect(invokeSelectorResult(beacon({ visible: true }), "tap")).toMatchObject({ visible: true });
		expect(invokeSelectorResult(beacon({ visible: false }), "tap")).toMatchObject({
			visible: false,
		});
		const a = invokeAnchorResult(
			{ tier: "anchored", highlighted: true, element: el },
			beacon({ visible: true }),
			"tap",
		);
		expect(a.visible).toBe(true);
	});
});

describe("reveal decision and wait", () => {
	it("switches only the room in front", () => {
		expect(revealDecision("r", "r")).toBe("switch");
		expect(revealDecision("x", "r")).toBe("other_room");
		expect(revealDecision(null, "r")).toBe("other_room");
	});
	it.each([
		["r", "r", true, "show_docked"],
		["r", "r", false, "switch"],
		["x", "r", true, "other_room"],
		[null, "r", true, "other_room"],
	] as const)("decision %s/%s docked=%s -> %s", (active, room, docked, want) => {
		expect(revealDecision(active, room, docked)).toBe(want);
	});
	it("revealPane shows a docked harness without switching", async () => {
		const switchHarness = vi.fn();
		const showDocked = vi.fn();
		const reveal = { activeRoomId: () => "r", switchHarness, showDocked, isDocked: () => true };
		expect(await revealPane({ whenVisible: async () => true }, reveal, "r", "h")).toBe(true);
		expect(showDocked).toHaveBeenCalledWith("r", "h");
		expect(switchHarness).not.toHaveBeenCalled();
	});
	it("pollUntil resolves true when met, false on timeout", async () => {
		let n = 0;
		expect(await pollUntil(() => ++n > 2, 500, 1)).toBe(true);
		expect(await pollUntil(() => false, 30, 5)).toBe(false);
	});
	it("revealPane switches in the front room and reports the wait", async () => {
		const switchHarness = vi.fn();
		const pane = { whenVisible: async () => true };
		expect(
			await revealPane(
				pane,
				{ activeRoomId: () => "r", switchHarness, showDocked: vi.fn(), isDocked: () => false },
				"r",
				"h",
			),
		).toBe(true);
		expect(switchHarness).toHaveBeenCalledWith("r", "h");
		const late = { whenVisible: async () => false };
		expect(
			await revealPane(
				late,
				{ activeRoomId: () => "r", switchHarness, showDocked: vi.fn(), isDocked: () => false },
				"r",
				"h",
			),
		).toBe(false);
	});
	it("revealPane switches nothing from another room", async () => {
		const switchHarness = vi.fn();
		const whenVisible = vi.fn(async () => true);
		expect(
			await revealPane(
				{ whenVisible },
				{ activeRoomId: () => "x", switchHarness, showDocked: vi.fn(), isDocked: () => false },
				"r",
				"h",
			),
		).toBe(false);
		expect(switchHarness).not.toHaveBeenCalled();
		expect(whenVisible).not.toHaveBeenCalled();
	});
});

describe("handleDesignRequest reveal", () => {
	const mkPane = (): DesignPaneApi => ({
		roomId: "r",
		ready: () => true,
		whenVisible: async () => true,
		getState: () => {
			throw new Error("unused");
		},
		showElement: async () => ({ tier: "selector", highlighted: true, element: el }),
		showChanges: async () => ({ highlighted: 0, mapped: [], unmapped: [], limits: "" }),
		invokeElement: async () => ({
			tier: "selector",
			invoked: true,
			action: "tap",
			element: el,
			visible: true,
		}),
	});
	const run = async (kind: string, args: object, active: string | null) => {
		const dispose = registerDesignPane("h", mkPane());
		const complete = vi.fn(async () => {});
		const switchHarness = vi.fn();
		await handleDesignRequest(kind, "id", args, [], vi.fn(), vi.fn(), complete, {
			activeRoomId: () => active,
			switchHarness,
			showDocked: vi.fn(),
			isDocked: () => false,
		});
		dispose();
		return {
			ok: (complete.mock.calls[0] as unknown[])[1] as Record<string, unknown>,
			switchHarness,
		};
	};
	it("adds revealed only when requested", async () => {
		const plain = await run("design.show_element", { ...base, selector: "p" }, "r");
		expect("revealed" in plain.ok).toBe(false);
		const yes = await run("design.show_element", { ...base, selector: "p", reveal: true }, "r");
		expect(yes.ok.revealed).toBe(true);
		const away = await run(
			"design.invoke_element",
			{ ...base, selector: "p", action: "tap", reveal: true },
			"other",
		);
		expect(away.ok.revealed).toBe(false);
		expect(away.switchHarness).not.toHaveBeenCalled();
	});
});
