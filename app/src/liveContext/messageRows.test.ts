// #329: the mailbox rows' pure label derivation (rows.tsx renders
// these strings, but the logic itself needs no React).

import { describe, expect, it } from "vitest";
import { messageInLabel, messageOutLabel } from "./payload.ts";
import type { Payload } from "./payload.ts";

describe("messageInLabel", () => {
	it("prefers the room and harness names", () => {
		const payload: Payload = {
			from_room_id: "room-1",
			from_room_name: "Alpha",
			from_harness_id: "h-1",
			from_harness_label: "claude",
		};
		expect(messageInLabel(payload)).toBe("message from Alpha · claude");
	});

	it("falls back to ids when names are missing", () => {
		const payload: Payload = { from_room_id: "room-1", from_harness_id: "h-1" };
		expect(messageInLabel(payload)).toBe("message from room-1 · h-1");
	});

	it("drops the harness clause entirely when no harness name or id survived", () => {
		const payload: Payload = { from_room_name: "Alpha" };
		expect(messageInLabel(payload)).toBe("message from Alpha");
	});

	it("doesn't crash on an empty payload", () => {
		expect(messageInLabel({})).toBe("message from ?");
	});
});

describe("messageOutLabel", () => {
	it("prefers the room and harness names", () => {
		const payload: Payload = {
			to_room_id: "room-2",
			to_room_name: "Beta",
			to_harness_id: "h-2",
			to_harness_label: "opencode",
		};
		expect(messageOutLabel(payload)).toBe("message to Beta · opencode");
	});

	it("falls back to ids when names are missing", () => {
		const payload: Payload = { to_room_id: "room-2", to_harness_id: "h-2" };
		expect(messageOutLabel(payload)).toBe("message to room-2 · h-2");
	});

	it("drops the harness clause entirely when no harness name or id survived", () => {
		const payload: Payload = { to_room_name: "Beta" };
		expect(messageOutLabel(payload)).toBe("message to Beta");
	});

	it("doesn't crash on an empty payload", () => {
		expect(messageOutLabel({})).toBe("message to ?");
	});
});
