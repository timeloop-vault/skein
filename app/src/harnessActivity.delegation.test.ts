import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";
import { subagents } from "./subagents.ts";

// #277 (epic #298) — the subagent-aware end-of-turn deferral. Its own
// file because the idle tick is a module-global `setInterval` started
// by the first `spawned()`: fake timers have to be installed before
// that happens, and vitest gives each test file a fresh module
// instance. Same convention as `harnessActivity.watchdog.test.ts`.

const nextId = (() => {
	let n = 0;
	return () => `d_${++n}`;
})();

/// A Claude harness as LiveTerminal + `attachClaudeEvents` leave it:
/// spawned, adapter attached and already running (a turn in
/// progress) — the state `awaiting_prompt` always arrives from.
const runningHarness = (): string => {
	const id = nextId();
	harnessActivity.spawned(id);
	harnessActivity.attachAuthoritativeSource(id);
	harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
	return id;
};

/// What the real translator does for a live `subagent_start`
/// (`event.initial !== true`): record it, then note the activity —
/// see `harnessEvents.ts`'s `subagent_start` arm.
const startLiveSubagent = (harnessId: string, agentId: string): void => {
	subagents.record(harnessId, { agentId, agentType: "explore", description: null }, false);
	harnessActivity.noteSubagentStarted(harnessId);
};

/// What the real translator does for `subagent_end`.
const finishSubagent = (harnessId: string, agentId: string): void => {
	subagents.finish(harnessId, agentId);
	harnessActivity.noteSubagentActivity(harnessId);
};

describe("subagent-aware end-of-turn deferral (#277)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});

	// Each harness created via `runningHarness()`/`nextId()` above must be
	// released at the end of its test: the idle tick's fake-timer sweep is
	// module-global, so a later test's large `advanceTimersByTime` would
	// otherwise re-evaluate an earlier test's still-deferred harness and
	// print a stray "presumed gone" `console.warn`. Assertions don't
	// change; this is cleanup only.

	it("awaiting_prompt with no working subagents goes straight to waiting, unchanged behaviour", () => {
		const id = runningHarness();
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("defers with one working subagent: phase stays running, no transition emitted", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");

		const transitions: Array<{ to: string; source: string }> = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, to, source) => {
			if (tid === id) transitions.push({ to, source });
		});

		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);

		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(harnessActivity.get(id)?.delegationDeferredAt).not.toBeNull();
		expect(transitions).toEqual([]);
		unsubscribe();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("a main-session work signal disarms the deferral; the next empty end-of-turn goes to waiting normally", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.delegationDeferredAt).not.toBeNull();

		// The dominant real-world path: the main session wakes up and
		// works the delegation's result.
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeToolUse);
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();

		finishSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("settle: deferred, subagent ends, resolves to waiting after DELEGATION_SETTLE_MS", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		finishSubagent(id, "a1");

		vi.advanceTimersByTime(59_000);
		expect(harnessActivity.get(id)?.phase).toBe("running");

		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});
		vi.advanceTimersByTime(2_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.DelegationSettled]);
		unsubscribe();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("ceiling: a working subagent that never reports again resolves to waiting and is presumed gone", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		// No finish, no tool result — the subagent just stops reporting.

		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});
		vi.advanceTimersByTime(15 * 60_000 + 2_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.DelegationCeiling]);
		expect(subagents.workingCount(id)).toBe(0);
		unsubscribe();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("adapter detach disarms an armed deferral: L2a regains control, not the delegation timers", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.delegationDeferredAt).not.toBeNull();

		// session_end: the adapter's file vanished. Real callers also
		// call `releasePermission` first; irrelevant here since the
		// harness isn't in `permission`.
		harnessActivity.detachAuthoritativeSource(id);
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
		expect(harnessActivity.get(id)?.authoritative).toBe(false);

		const sources: string[] = [];
		const unsubscribe = harnessActivity.subscribeTransitions((tid, _from, _to, source) => {
			if (tid === id) sources.push(source);
		});

		// `setRunningFromAdapter` never touches `lastOutputAt` — only
		// `recordOutput` does — so prime it the way a real PTY chunk
		// would right after detach, same phase so no transition fires.
		harnessActivity.recordOutput(id, "ordinary output\n");
		expect(sources).toEqual([]);

		// No further PTY output: past IDLE_AFTER_MS, plain L2a — not a
		// leftover deferral — is what flips this to idle.
		vi.advanceTimersByTime(8_500);
		expect(harnessActivity.get(id)?.phase).toBe("idle");
		expect(sources).toEqual([TRANSITION_SOURCE.L2aIdle]);

		// A chunk of PTY output moves it straight back to running —
		// L2a is reading the PTY again, not stuck behind a dead adapter.
		harnessActivity.recordOutput(id, "still here\n");
		expect(harnessActivity.get(id)?.phase).toBe("running");

		// Advance well past DELEGATION_CEILING_MS with no further
		// output. Had the deferral survived the detach, Rule 4 would
		// flush this to "waiting" via DelegationCeiling once
		// `delegationActivityAt` (fed only by the same dead adapter's
		// subagent events) went stale; disarmed, the tick never
		// evaluates the deferral branch at all, so only plain L2a idle
		// detection fires.
		vi.advanceTimersByTime(15 * 60_000 + 2_000);
		expect(harnessActivity.get(id)?.phase).toBe("idle");
		expect(sources).toEqual([
			TRANSITION_SOURCE.L2aIdle,
			TRANSITION_SOURCE.PtyOutput,
			TRANSITION_SOURCE.L2aIdle,
		]);
		expect(sources).not.toContain(TRANSITION_SOURCE.DelegationCeiling);
		expect(sources).not.toContain(TRANSITION_SOURCE.DelegationSettled);

		unsubscribe();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("an attach-time subagent never defers anything", () => {
		const id = runningHarness();
		// initial: true — an attach-time replay, not a live start.
		subagents.record(id, { agentId: "orphan", agentType: "explore", description: null }, true);

		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);

		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("permission outranks both timers; settle still resolves once the dialog clears", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		finishSubagent(id, "a1");

		// A permission dialog opens on the main session mid-deferral.
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		expect(harnessActivity.get(id)?.phase).toBe("permission");

		// Well past the settle threshold — the tick must leave it alone
		// while the dialog is open.
		vi.advanceTimersByTime(70_000);
		expect(harnessActivity.get(id)?.phase).toBe("permission");
		expect(harnessActivity.get(id)?.delegationDeferredAt).not.toBeNull();

		// Dialog answered — clears to running; the deferral timers apply
		// again from here.
		harnessActivity.clearPermission(id, TRANSITION_SOURCE.L2c1ClaudeSubagentToolResult, null);
		expect(harnessActivity.get(id)?.phase).toBe("running");

		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("delegatedCount counts live starts and resets on noteUserPrompt", () => {
		const id = runningHarness();
		startLiveSubagent(id, "a1");
		startLiveSubagent(id, "a2");
		expect(harnessActivity.get(id)?.delegatedCount).toBe(2);

		// An attach-time replay must not count.
		subagents.record(id, { agentId: "orphan", agentType: "explore", description: null }, true);
		expect(harnessActivity.get(id)?.delegatedCount).toBe(2);

		harnessActivity.noteUserPrompt(id);
		expect(harnessActivity.get(id)?.delegatedCount).toBe(0);
		harnessActivity.forget(id);
		subagents.forget(id);
	});
});
