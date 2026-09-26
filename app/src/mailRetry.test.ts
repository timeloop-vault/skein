import { describe, expect, it } from "vitest";
import { automaticGate, decideMailNudge } from "./mailNudge.ts";
import type { DecideMailNudgeInput } from "./mailNudge.ts";
import { MAIL_RETRY_WINDOW_MS, mailPending, nextMailRetry } from "./mailRetry.ts";
import type { NextMailRetryInput } from "./mailRetry.ts";

const retryInput = (over: Partial<NextMailRetryInput> = {}): NextMailRetryInput => ({
	pending: true,
	phase: "waiting",
	armedAtMs: 0,
	nowMs: 0,
	...over,
});

describe("nextMailRetry", () => {
	it("waits at the interval when pending and waiting", () => {
		expect(nextMailRetry(retryInput())).toEqual({ kind: "wait", delayMs: 2000 });
	});

	it("keeps trying while spawning — the harness may not be ready yet", () => {
		expect(nextMailRetry(retryInput({ phase: "spawning" }))).toEqual({
			kind: "wait",
			delayMs: 2000,
		});
	});

	it("keeps trying during permission — the dialog may close any moment", () => {
		expect(nextMailRetry(retryInput({ phase: "permission" }))).toEqual({
			kind: "wait",
			delayMs: 2000,
		});
	});

	it("keeps trying while running — a #277 deferral may already be armed", () => {
		expect(nextMailRetry(retryInput({ phase: "running" }))).toEqual({
			kind: "wait",
			delayMs: 2000,
		});
	});

	it("stops when nothing is pending any more", () => {
		expect(nextMailRetry(retryInput({ pending: false }))).toEqual({ kind: "stop" });
	});

	it("stops once the harness has exited", () => {
		expect(nextMailRetry(retryInput({ phase: "exited" }))).toEqual({ kind: "stop" });
	});

	it("stops when there's no activity record at all", () => {
		expect(nextMailRetry(retryInput({ phase: null }))).toEqual({ kind: "stop" });
	});

	it("stops once the retry window has elapsed", () => {
		expect(nextMailRetry(retryInput({ armedAtMs: 0, nowMs: MAIL_RETRY_WINDOW_MS + 1 }))).toEqual({
			kind: "stop",
		});
	});

	it("stops exactly at the window boundary — >= , not >", () => {
		expect(nextMailRetry(retryInput({ armedAtMs: 0, nowMs: MAIL_RETRY_WINDOW_MS }))).toEqual({
			kind: "stop",
		});
	});

	it("still waits just under the window boundary", () => {
		expect(nextMailRetry(retryInput({ armedAtMs: 0, nowMs: MAIL_RETRY_WINDOW_MS - 1 }))).toEqual({
			kind: "wait",
			delayMs: 2000,
		});
	});
});

describe("mailPending", () => {
	it("is pending when unread has grown past what was last nudged", () => {
		expect(mailPending(2, 0)).toBe(true);
	});

	it("is not pending once nudged for the current count", () => {
		expect(mailPending(2, 2)).toBe(false);
	});

	it("is never pending for zero unread", () => {
		expect(mailPending(0, 0)).toBe(false);
	});
});

// Scenario tests composing the real pure pieces the way `useMailDelivery.ts`
// itself does — `canSendPrompt` is not imported here to keep this module's
// tests DOM/store-free; instead the `GateResult` a caller would have
// gotten from it is supplied directly, exactly the split `mailNudge.ts`
// itself already draws.

const GATE_OK = { ok: true } as const;
const gateRefused = (reason: string) => ({ ok: false, reason }) as const;

const decide = (over: Partial<DecideMailNudgeInput>) =>
	decideMailNudge({
		atStoppingPoint: true,
		unread: 1,
		lastNudged: 0,
		gate: GATE_OK,
		...over,
	});

describe("scenario: mail queued during spawning, the waiting transition missed (#386)", () => {
	it("a later timer tick at waiting, seam registered, nudges once, then the retry stops", () => {
		// Arrival while spawning: not at a stopping point, gate refused —
		// nothing recorded, matching `decideMailNudge`'s own contract.
		const whileSpawning = decide({
			atStoppingPoint: false,
			unread: 2,
			lastNudged: 0,
			gate: gateRefused("this harness isn't ready yet"),
		});
		expect(whileSpawning).toEqual({ nudge: false, lastNudged: 0 });
		expect(mailPending(2, whileSpawning.lastNudged)).toBe(true);
		expect(nextMailRetry(retryInput({ pending: true, phase: "spawning" }))).toEqual({
			kind: "wait",
			delayMs: 2000,
		});

		// Seam registers, harness is now `waiting` — the seam-registered
		// trigger runs `check` again, gate now passes.
		const atWaiting = decide({
			atStoppingPoint: true,
			unread: 2,
			lastNudged: whileSpawning.lastNudged,
			gate: GATE_OK,
		});
		expect(atWaiting).toEqual({ nudge: true, lastNudged: 2 });
		const pendingAfterNudge = mailPending(2, atWaiting.lastNudged);
		expect(pendingAfterNudge).toBe(false);
		expect(nextMailRetry(retryInput({ pending: pendingAfterNudge, phase: "waiting" }))).toEqual({
			kind: "stop",
		});

		// Next tick, same count — does not nudge again.
		const secondTick = decide({
			atStoppingPoint: true,
			unread: 2,
			lastNudged: atWaiting.lastNudged,
			gate: GATE_OK,
		});
		expect(secondTick).toEqual({ nudge: false, lastNudged: 2 });
	});
});

describe("scenario: held by a draft (#383) until it clears", () => {
	it("refuses while a draft is present, retries, then nudges once cleared", () => {
		const held = decide({
			unread: 3,
			lastNudged: 0,
			gate: automaticGate(GATE_OK, { kind: "typed", chars: 4 }),
		});
		expect(held).toEqual({ nudge: false, lastNudged: 0 });
		expect(
			nextMailRetry(retryInput({ pending: mailPending(3, held.lastNudged), phase: "waiting" })),
		).toEqual({ kind: "wait", delayMs: 2000 });

		const cleared = decide({
			unread: 3,
			lastNudged: held.lastNudged,
			gate: automaticGate(GATE_OK, { kind: "clean" }),
		});
		expect(cleared).toEqual({ nudge: true, lastNudged: 3 });
		expect(mailPending(3, cleared.lastNudged)).toBe(false);
	});
});

describe("scenario: permission phase never nudges, retry keeps waiting", () => {
	it("does not nudge while at a permission dialog", () => {
		const decision = decide({
			atStoppingPoint: false,
			unread: 1,
			lastNudged: 0,
			gate: gateRefused("the harness is waiting on a permission dialog"),
		});
		expect(decision).toEqual({ nudge: false, lastNudged: 0 });
		expect(
			nextMailRetry(
				retryInput({ pending: mailPending(1, decision.lastNudged), phase: "permission" }),
			),
		).toEqual({ kind: "wait", delayMs: 2000 });
	});
});

describe("scenario: never pastes over a draft even at waiting", () => {
	it("holds at waiting when the draft is typed, gate would otherwise pass", () => {
		const decision = decide({
			atStoppingPoint: true,
			unread: 1,
			lastNudged: 0,
			gate: automaticGate(GATE_OK, { kind: "typed", chars: 1 }),
		});
		expect(decision).toEqual({ nudge: false, lastNudged: 0 });
	});
});
