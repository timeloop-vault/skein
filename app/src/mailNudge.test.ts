import { describe, expect, it } from "vitest";
import type { ComposerDraft } from "./composerDraft.ts";
import {
	automaticGate,
	decideMailNudge,
	mailNudgeText,
	mailPopoverText,
	shouldCheckOnTransition,
} from "./mailNudge.ts";
import type { DecideMailNudgeInput } from "./mailNudge.ts";

const CLEAN: ComposerDraft = { kind: "clean" };
const TYPED: ComposerDraft = { kind: "typed", chars: 3 };
const UNKNOWN: ComposerDraft = { kind: "unknown" };

const GATE_OK = { ok: true } as const;
const GATE_CLOSED = { ok: false, reason: "not ready" } as const;

const input = (over: Partial<DecideMailNudgeInput> = {}): DecideMailNudgeInput => ({
	atStoppingPoint: true,
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

	// #381: `atStoppingPoint` collapses `waiting` and a deferred `running`
	// into one bool at this layer — the caller (`atSafeStoppingPoint`,
	// tested separately in harnessActivityCore.test.ts) is what tells
	// `running` with a deferral armed apart from plain `running` or
	// `permission`, both of which pass `false` here.
	it("does not nudge, and records nothing, when not at a safe stopping point (running, no deferral)", () => {
		expect(decideMailNudge(input({ atStoppingPoint: false, unread: 3, lastNudged: 1 }))).toEqual({
			nudge: false,
			lastNudged: 1,
		});
	});

	it("does not nudge, and records nothing, when not at a safe stopping point (permission)", () => {
		expect(decideMailNudge(input({ atStoppingPoint: false, unread: 2, lastNudged: 0 }))).toEqual({
			nudge: false,
			lastNudged: 0,
		});
	});

	it("nudges when running with a #277 delegation deferral armed", () => {
		expect(decideMailNudge(input({ atStoppingPoint: true, unread: 3, lastNudged: 1 }))).toEqual({
			nudge: true,
			lastNudged: 3,
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

describe("automaticGate (#383)", () => {
	it("passes a clean draft through unchanged", () => {
		expect(automaticGate(GATE_OK, CLEAN)).toEqual(GATE_OK);
	});

	it("holds on a typed draft", () => {
		expect(automaticGate(GATE_OK, TYPED).ok).toBe(false);
	});

	it("holds on an unknown draft, same as typed", () => {
		expect(automaticGate(GATE_OK, UNKNOWN).ok).toBe(false);
	});

	it("keeps the underlying gate's own reason when it's already refused, even with a clean draft", () => {
		expect(automaticGate(GATE_CLOSED, CLEAN)).toEqual(GATE_CLOSED);
	});
});

describe("decideMailNudge with automaticGate folded in (#383)", () => {
	it("a typed draft holds — nudge:false, lastNudged unchanged, so it retries later", () => {
		const gate = automaticGate(GATE_OK, TYPED);
		expect(decideMailNudge(input({ unread: 2, lastNudged: 0, gate }))).toEqual({
			nudge: false,
			lastNudged: 0,
		});
	});

	it("a clean draft nudges", () => {
		const gate = automaticGate(GATE_OK, CLEAN);
		expect(decideMailNudge(input({ unread: 2, lastNudged: 0, gate }))).toEqual({
			nudge: true,
			lastNudged: 2,
		});
	});

	it("an unknown draft holds", () => {
		const gate = automaticGate(GATE_OK, UNKNOWN);
		expect(decideMailNudge(input({ unread: 2, lastNudged: 0, gate }))).toEqual({
			nudge: false,
			lastNudged: 0,
		});
	});

	it("held while typed, then nudges once the draft clears — same unread count both times", () => {
		const held = decideMailNudge(
			input({ unread: 2, lastNudged: 0, gate: automaticGate(GATE_OK, TYPED) }),
		);
		expect(held).toEqual({ nudge: false, lastNudged: 0 });

		const cleared = decideMailNudge(
			input({ unread: 2, lastNudged: held.lastNudged, gate: automaticGate(GATE_OK, CLEAN) }),
		);
		expect(cleared).toEqual({ nudge: true, lastNudged: 2 });
	});

	it("an underlying gate refusal wins over a clean draft", () => {
		const decision = decideMailNudge(
			input({ unread: 2, lastNudged: 0, gate: automaticGate(GATE_CLOSED, CLEAN) }),
		);
		expect(decision).toEqual({ nudge: false, lastNudged: 0 });
	});
});

describe("shouldCheckOnTransition (#381)", () => {
	it("always checks on a transition into waiting", () => {
		expect(shouldCheckOnTransition("waiting", false)).toBe(true);
		expect(shouldCheckOnTransition("waiting", true)).toBe(true);
	});

	it("checks a transition to running when now at a safe stopping point (a deferral is armed)", () => {
		expect(shouldCheckOnTransition("running", true)).toBe(true);
	});

	it("does not check a transition to running with no deferral armed", () => {
		expect(shouldCheckOnTransition("running", false)).toBe(false);
	});

	it("does not check a transition to permission", () => {
		// `atStoppingPointNow` is derived from `atSafeStoppingPoint`, which
		// is never true while `phase === "permission"` — only the `false`
		// case is a real combination.
		expect(shouldCheckOnTransition("permission", false)).toBe(false);
	});

	it("does not check a transition to idle or exited", () => {
		expect(shouldCheckOnTransition("idle", false)).toBe(false);
		expect(shouldCheckOnTransition("exited", false)).toBe(false);
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
