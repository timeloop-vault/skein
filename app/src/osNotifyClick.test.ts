import { describe, expect, it } from "vitest";
import { resolveClickTarget, shouldDrainOnClickEvent } from "./osNotifyClick.ts";
import type { Room } from "./types.ts";

const room = (id: string, harnessIds: string[]): Room => ({
	id,
	name: id,
	task: "",
	status: "idle",
	badge: 0,
	harnesses: harnessIds.map((hid) => ({
		id: hid,
		kind: "claude",
		name: hid,
		status: "idle",
		model: "",
		tokens: "",
	})),
	activeHarnessId: harnessIds[0] ?? "",
});

describe("resolveClickTarget", () => {
	it("reports not found when the room was closed and deleted since the banner fired", () => {
		const rooms = [room("r1", ["h1"])];
		expect(resolveClickTarget(rooms, { roomId: "gone", harnessId: "h1" })).toEqual({
			found: false,
		});
	});

	it("finds the room and confirms the harness still belongs to it", () => {
		const rooms = [room("r1", ["h1", "h2"])];
		expect(resolveClickTarget(rooms, { roomId: "r1", harnessId: "h2" })).toEqual({
			found: true,
			hasHarness: true,
		});
	});

	it("finds the room but reports the harness gone", () => {
		const rooms = [room("r1", ["h1"])];
		expect(resolveClickTarget(rooms, { roomId: "r1", harnessId: "deleted" })).toEqual({
			found: true,
			hasHarness: false,
		});
	});
});

describe("shouldDrainOnClickEvent", () => {
	it("ignores the event while rooms aren't loaded yet — the post-load drain owns it", () => {
		expect(shouldDrainOnClickEvent(false)).toBe(false);
	});

	it("drains once rooms have loaded", () => {
		expect(shouldDrainOnClickEvent(true)).toBe(true);
	});
});
