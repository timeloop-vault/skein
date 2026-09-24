import { describe, expect, it } from "vitest";
import { decideMailNudge, mailNudgeText, mailPopoverText } from "./mailNudge.ts";
import type { DecideMailNudgeInput } from "./mailNudge.ts";

const GATE_OK = { ok: true } as const;
const GATE_CLOSED = { ok: false, reason: "not ready" } as const;

const input = (over: Partial<DecideMailNudgeInput> = {}): DecideMailNudgeInput => ({
	phase: "waiting",
	unread: 1,
	lastNudged: 0,
	gate: GATE_OK,
	...over,
});

describe("decideMailNudge", () => {
	it("nudges when unread has grown past what was last nudged", () => {
		expect(decideMailNudge(input({ unread: 2, lastNudged: 0 }))).toEqual({
			nudge: true,
			lastNudged: 2,
		});
	});

	it("does not nudge again for the same count", () => {
		expect(decideMailNudge(input({ unread: 2, lastNudged: 2 }))).toEqual({
			nudge: false,
			lastNudged: 2,
		});
	});

	it("does not nudge, and records nothing, when the phase isn't waiting", () => {
		expect(decideMailNudge(input({ phase: "running", unread: 3, lastNudged: 1 }))).toEqual({
			nudge: false,
			lastNudged: 1,
		});
	});

	it("does not nudge, and records nothing, when the gate is closed", () => {
		expect(decideMailNudge(input({ gate: GATE_CLOSED, unread: 3, lastNudged: 1 }))).toEqual({
			nudge: false,
			lastNudged: 1,
		});
	});

	it("a full read resets the remembered count, so new mail nudges again", () => {
		const afterRead = decideMailNudge(input({ unread: 0, lastNudged: 3 }));
		expect(afterRead).toEqual({ nudge: false, lastNudged: 0 });

		const afterNewMail = decideMailNudge(input({ unread: 1, lastNudged: afterRead.lastNudged }));
		expect(afterNewMail).toEqual({ nudge: true, lastNudged: 1 });
	});

	it("a partial read only nudges again once unread exceeds what's left", () => {
		// 3 already nudged, user reads down to 1 unread.
		const afterPartialRead = decideMailNudge(input({ unread: 1, lastNudged: 3 }));
		expect(afterPartialRead).toEqual({ nudge: false, lastNudged: 1 });

		// One more unread arrives (2 total) — that's growth past 1, nudge.
		const afterMore = decideMailNudge(
			input({ unread: 2, lastNudged: afterPartialRead.lastNudged }),
		);
		expect(afterMore).toEqual({ nudge: true, lastNudged: 2 });
	});

	it("after a restart (lastNudged 0) with mail already stored, nudges once on first waiting", () => {
		const first = decideMailNudge(input({ unread: 2, lastNudged: 0 }));
		expect(first).toEqual({ nudge: true, lastNudged: 2 });

		const second = decideMailNudge(input({ unread: 2, lastNudged: first.lastNudged }));
		expect(second).toEqual({ nudge: false, lastNudged: 2 });
	});

	it("never nudges for zero unread", () => {
		expect(decideMailNudge(input({ unread: 0, lastNudged: 0 }))).toEqual({
			nudge: false,
			lastNudged: 0,
		});
	});
});

describe("mailNudgeText", () => {
	it("uses singular wording for one message, no room names", () => {
		expect(mailNudgeText(1, [])).toBe("You have 1 new message in Skein. Call read_messages.");
	});

	it("uses plural wording for more than one message", () => {
		expect(mailNudgeText(3, [])).toBe("You have 3 new messages in Skein. Call read_messages.");
	});

	it("names a single room", () => {
		expect(mailNudgeText(2, ["Alpha"])).toBe(
			"You have 2 new messages in Skein from Alpha. Call read_messages.",
		);
	});

	it("joins multiple room names with a comma", () => {
		expect(mailNudgeText(2, ["Alpha", "Beta"])).toBe(
			"You have 2 new messages in Skein from Alpha, Beta. Call read_messages.",
		);
	});
});

describe("mailPopoverText", () => {
	it("uses singular wording for one message, no room names", () => {
		expect(mailPopoverText(1, [])).toBe("1 unread message");
	});

	it("uses plural wording for more than one message", () => {
		expect(mailPopoverText(3, [])).toBe("3 unread messages");
	});

	it("names a single room", () => {
		expect(mailPopoverText(1, ["test-5"])).toBe("1 unread message from test-5");
	});

	it("joins multiple room names with a comma", () => {
		expect(mailPopoverText(2, ["Alpha", "Beta"])).toBe("2 unread messages from Alpha, Beta");
	});
});
