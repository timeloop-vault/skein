import { describe, expect, it } from "vitest";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import {
	classifyTransition,
	clearHarnessPending,
	dropToastsFor,
	isNotifiable,
	osNotificationLabel,
	setHarnessPending,
	toastState,
} from "./harnessNotifyLogic.ts";
import type { ToastEntry } from "./toastStack.ts";
import type { Room } from "./types.ts";

const room = (pending: number): Room =>
	({
		id: "r1",
		harnesses: [
			{ id: "h1", pendingNotifications: pending },
			{ id: "h2", pendingNotifications: 0 },
		],
	}) as unknown as Room;

describe("isNotifiable", () => {
	it("excludes spawning -> waiting", () => {
		expect(isNotifiable(classifyTransition("spawning", "waiting"))).toBe(false);
	});
	it("includes running -> waiting, any -> permission, running -> idle", () => {
		expect(isNotifiable(classifyTransition("running", "waiting"))).toBe(true);
		expect(isNotifiable(classifyTransition("spawning", "permission"))).toBe(true);
		expect(isNotifiable(classifyTransition("running", "idle"))).toBe(true);
		expect(isNotifiable(classifyTransition("idle", "running"))).toBe(false);
	});
});

describe("isNotifiable source (#175)", () => {
	it("never notifies for the opencode reconnect baseline, still does for a real idle", () => {
		const c = classifyTransition("running", "waiting");
		expect(isNotifiable(c, TRANSITION_SOURCE.L2c2OpencodeBaseline)).toBe(false);
		expect(isNotifiable(c, TRANSITION_SOURCE.L2c2OpencodeIdle)).toBe(true);
	});
});

describe("toastState / osNotificationLabel", () => {
	it("maps phases", () => {
		expect(toastState("waiting")).toBe("waiting");
		expect(toastState("permission")).toBe("permission");
		expect(toastState("idle")).toBe("idle");
		expect(toastState("exited")).toBe("exited");
	});
	it("words permission and delegation notes", () => {
		expect(osNotificationLabel("permission", null, "a", "Bash")).toBe(
			"needs permission (a · Bash)",
		);
		expect(osNotificationLabel("permission", null, null, null)).toBe("needs permission");
		expect(osNotificationLabel("waiting", "2 done", null, null)).toBe("waiting (2 done)");
		expect(osNotificationLabel("idle", "x", null, null)).toBe("idle");
	});
});

describe("pending reducers", () => {
	it("sets and clears one harness", () => {
		const set = setHarnessPending([room(0)], "h1");
		expect(set[0]?.harnesses[0]?.pendingNotifications).toBe(1);
		const cleared = clearHarnessPending(set, "r1", "h1");
		expect(cleared[0]?.harnesses[0]?.pendingNotifications).toBe(0);
	});
	it("keeps identity when nothing to clear", () => {
		const rooms = [room(0)];
		expect(clearHarnessPending(rooms, "r1", "h1")[0]).toBe(rooms[0]);
	});
});

describe("dropToastsFor", () => {
	const t = (roomId: string, harnessId: string) =>
		({ id: harnessId, roomId, harnessId }) as ToastEntry;
	it("returns same array when nothing matches", () => {
		const prev = [t("r1", "h1")];
		expect(dropToastsFor(prev, "r1", "h2")).toBe(prev);
		expect(dropToastsFor(prev, "r1", "h1")).toEqual([]);
	});
});
