import { describe, expect, it, vi } from "vitest";
import { handleDesignRequest } from "./designAgentRequests.ts";
import {
	captureTargetResult,
	intersectClips,
	notVisibleReason,
	type PaneCaptureRaw,
	parseHarnessIdArgs,
	showDesignPane,
	showPaneDecision,
} from "./designCapture.ts";
import { type DesignPaneApi, registerDesignPane } from "./designControl.ts";

const rect = { x: 10, y: 20, width: 300, height: 600 };
const raw = (o: Partial<PaneCaptureRaw> = {}): PaneCaptureRaw => ({
	visible: true,
	laidOut: true,
	rect,
	cssHidden: false,
	devicePixelRatio: 2,
	...o,
});

describe("intersectClips", () => {
	const vp = { x: 0, y: 0, width: 1000, height: 800 };
	it("returns the rect when nothing clips it", () => {
		expect(intersectClips(rect, [vp])).toEqual(rect);
	});
	it("clips to an ancestor and the viewport", () => {
		const anc = { x: 0, y: 0, width: 100, height: 100 };
		expect(intersectClips(rect, [vp, anc])).toEqual({ x: 10, y: 20, width: 90, height: 80 });
		expect(intersectClips({ x: 900, y: 700, width: 300, height: 300 }, [vp])).toEqual({
			x: 900,
			y: 700,
			width: 100,
			height: 100,
		});
	});
	it("is null when scrolled out, collapsed or under 1px", () => {
		expect(intersectClips({ ...rect, y: 900 }, [vp])).toBeNull();
		expect(intersectClips(rect, [vp, { x: 10, y: 20, width: 0, height: 50 }])).toBeNull();
		expect(intersectClips(rect, [{ x: 0, y: 0, width: 10.5, height: 800 }])).toBeNull();
	});
});

describe("notVisibleReason", () => {
	it("is null only when everything lines up", () => {
		expect(notVisibleReason("r", "r", raw())).toBeNull();
	});
	it.each([
		["other room wins", "x", raw({ visible: false }), "other_room"],
		["no active room", null, raw(), "other_room"],
		["tab not shown", "r", raw({ visible: false }), "hidden_tab"],
		["visibility hidden", "r", raw({ cssHidden: true }), "hidden_tab"],
		["not laid out", "r", raw({ laidOut: false }), "not_laid_out"],
		["zero rect", "r", raw({ rect: { ...rect, width: 0 } }), "not_laid_out"],
		["no rect", "r", raw({ rect: null }), "not_laid_out"],
	] as const)("%s -> %s", (_n, active, r, want) => {
		expect(notVisibleReason(active, "r", r)).toBe(want);
	});
});

describe("captureTargetResult", () => {
	it("reports rect, dpr and entry when painted", () => {
		expect(captureTargetResult("r", "r", raw(), "a.html")).toEqual({
			visible: true,
			rect,
			devicePixelRatio: 2,
			entry: "a.html",
		});
	});
	it("reports a reason and no rect otherwise", () => {
		const r = captureTargetResult("x", "r", raw(), "a.html");
		expect(r).toEqual({ visible: false, devicePixelRatio: 2, reason: "other_room" });
	});
});

describe("parseHarnessIdArgs", () => {
	it("requires a non-empty harnessId", () => {
		expect(parseHarnessIdArgs({ harnessId: "h" })).toEqual({ ok: true, value: { harnessId: "h" } });
		for (const bad of [null, {}, { harnessId: "" }, { harnessId: 3 }]) {
			expect(parseHarnessIdArgs(bad).ok).toBe(false);
		}
	});
});

describe("showPaneDecision", () => {
	it.each([
		["r", "r", false, true, "already", false, null],
		["r", "r", false, false, "switch", false, "switch"],
		["r", "r", true, false, "show_docked", false, "show_docked"],
		["x", "r", false, false, "room", true, "switch"],
		["x", "r", true, false, "room", true, "show_docked"],
		[null, "r", false, false, "room", true, "switch"],
	] as const)("%s/%s docked=%s visible=%s -> %s", (active, room, docked, vis, how, sw, h) => {
		expect(showPaneDecision(active, room, docked, vis)).toEqual({
			how,
			switchRoom: sw,
			harness: h,
		});
	});
});

describe("showDesignPane", () => {
	const mk = (active: string | null, docked: boolean) => ({
		activeRoomId: () => active,
		switchRoom: vi.fn(),
		switchHarness: vi.fn(),
		isDocked: () => docked,
		showDocked: vi.fn(),
	});
	it("switches room, then the harness, and reports shown", async () => {
		const a = mk("x", false);
		const out = await showDesignPane({ roomId: "r", whenVisible: async () => true }, a, "h");
		expect(out).toEqual({ shown: true, switchedRoom: true, revealed: "room" });
		expect(a.switchRoom).toHaveBeenCalledWith("r");
		expect(a.switchHarness).toHaveBeenCalledWith("r", "h");
	});
	it("does nothing when already visible", async () => {
		const a = mk("r", false);
		const out = await showDesignPane({ roomId: "r", whenVisible: async () => true }, a, "h");
		expect(out).toEqual({ shown: true, switchedRoom: false, revealed: "already" });
		expect(a.switchRoom).not.toHaveBeenCalled();
		expect(a.switchHarness).not.toHaveBeenCalled();
	});
	it("shows a docked harness and reports a timeout", async () => {
		const a = mk("r", true);
		let first = true;
		const whenVisible = async () => {
			if (first) {
				first = false;
				return false;
			}
			return false;
		};
		const out = await showDesignPane({ roomId: "r", whenVisible }, a, "h");
		expect(out).toMatchObject({ shown: false, switchedRoom: false, revealed: "show_docked" });
		expect(out.reason).toMatch(/^timeout/);
		expect(a.showDocked).toHaveBeenCalledWith("r", "h");
		expect(a.switchHarness).not.toHaveBeenCalled();
	});
});

describe("handleDesignRequest capture / show", () => {
	const pane = (): DesignPaneApi => ({
		roomId: "r",
		ready: () => true,
		whenVisible: async () => true,
		captureTarget: () => raw(),
		getState: () => ({
			entry: "a.html",
			device: null,
			ready: true,
			loadFailed: false,
			errors: [],
			selected: null,
			scroll: null,
		}),
		showElement: async () => {
			throw new Error("unused");
		},
		invokeElement: async () => {
			throw new Error("unused");
		},
		showChanges: async () => {
			throw new Error("unused");
		},
	});
	const run = async (kind: string, args: object, mounted: boolean, active: string | null) => {
		const dispose = mounted ? registerDesignPane("h", pane()) : () => {};
		const complete = vi.fn(async () => {});
		await handleDesignRequest(kind, "id", args, [], vi.fn(), vi.fn(), complete, {
			activeRoomId: () => active,
			switchRoom: vi.fn(),
			switchHarness: vi.fn(),
			showDocked: vi.fn(),
			isDocked: () => false,
		});
		dispose();
		return complete.mock.calls[0] as unknown[];
	};
	it("capture_target answers visible, other_room and no_pane", async () => {
		const ok = (await run("design.capture_target", { harnessId: "h" }, true, "r"))[1];
		expect(ok).toMatchObject({ visible: true, rect, entry: "a.html" });
		const away = (await run("design.capture_target", { harnessId: "h" }, true, "x"))[1];
		expect(away).toMatchObject({ visible: false, reason: "other_room" });
		const none = (await run("design.capture_target", { harnessId: "h" }, false, "r"))[1];
		expect(none).toMatchObject({ visible: false, reason: "no_pane" });
	});
	it("show_pane errors when no pane is mounted", async () => {
		const call = await run("design.show_pane", { harnessId: "h" }, false, "r");
		expect(call[2]).toMatch(/^not_mounted/);
	});
});
