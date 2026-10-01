import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// #440 — a subagent's exit (classic end_turn OR a SubagentHandback
// tool_result, which Rust now reports as the very same `subagent_end`
// event) must release the #277 delegation deferral, and with it the
// held mail/nudge. Driven through the real `attachClaudeEvents`
// translator on a mocked Channel, so the wire shape is what the Rust
// tail emits. Own file: the idle tick is a module-global interval, so
// fake timers must be installed before the first `spawned()`.

const h = vi.hoisted(() => {
	const channels: Array<{ onmessage: (e: unknown) => void }> = [];
	return { channels };
});

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
		constructor() {
			h.channels.push(this);
		}
	},
	invoke: () => new Promise<void>(() => {}),
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { HARNESS_KINDS } from "./data.tsx";
import { atSafeStoppingPoint, harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { statusLabel } from "./harnessActivityLabels.ts";
import { attachClaudeEvents } from "./harnessEvents.ts";
import { canSendPrompt } from "./harnessInput.ts";
import { decideMailNudge } from "./mailNudge.ts";
import { subagents } from "./subagents.ts";

let n = 0;
const send = (event: unknown) => h.channels[h.channels.length - 1]?.onmessage(event);

/// A Claude harness mid-turn with its tail attached, as LiveTerminal
/// leaves it, with #215 injection on so the send gate can pass.
const attached = (): string => {
	const id = `se_${++n}`;
	harnessActivity.spawned(id);
	attachClaudeEvents(id, "room", "sid", "/cwd");
	harnessActivity.setInjected(id, true);
	send({ kind: "assistant_turn" });
	return id;
};

const startSub = (): void =>
	send({
		kind: "subagent_start",
		agent_id: "a1",
		agent_type: "explore",
		description: null,
		initial: false,
	});
const endSub = (): void =>
	send({ kind: "subagent_end", agent_id: "a1", agent_type: "explore", description: null });

const label = (id: string): string =>
	statusLabel(
		harnessActivity.get(id)?.phase === "running" ? "running" : "waiting",
		null,
		null,
		subagents.workingCount(id),
	);

describe("subagent exit releases the delegation deferral (#440)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});
	beforeEach(() => {
		h.channels.length = 0;
	});

	it("subagent_end after a deferred end-of-turn settles to waiting after DELEGATION_SETTLE_MS", () => {
		const id = attached();
		startSub();
		send({ kind: "awaiting_prompt" });
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(label(id)).toBe("delegating · 1 agent");

		endSub();
		expect(subagents.workingCount(id)).toBe(0);
		expect(label(id)).toBe("running");

		const sources: string[] = [];
		const unsub = harnessActivity.subscribeTransitions((tid, _f, _t, source) => {
			if (tid === id) sources.push(source);
		});
		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.DelegationSettled]);
		expect(label(id)).toBe("waiting");
		unsub();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("the main session's own next end-of-turn after subagent_end goes straight to waiting", () => {
		const id = attached();
		startSub();
		send({ kind: "awaiting_prompt" });
		endSub();
		// The main session wakes to work the result, then ends its turn.
		send({ kind: "assistant_turn" });
		send({ kind: "awaiting_prompt" });
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(harnessActivity.get(id)?.delegationDeferredAt).toBeNull();
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("held mail: delivered at arming (#381) and still deliverable once the exit settles the deferral", () => {
		const id = attached();
		// Seed the proof-of-life the send gate needs, as a real tail would.
		harnessActivity.adapterDelivered(id);
		startSub();
		send({ kind: "awaiting_prompt" });

		const gate = () =>
			canSendPrompt({
				capabilities: HARNESS_KINDS.claude.capabilities,
				activity: harnessActivity.get(id),
				registered: true,
				bracketedPasteOn: false,
				body: "mail",
			});
		const decide = (lastNudged: number) =>
			decideMailNudge({
				atStoppingPoint: atSafeStoppingPoint(
					harnessActivity.get(id) as NonNullable<ReturnType<typeof harnessActivity.get>>,
				),
				unread: 1,
				lastNudged,
				gate: gate(),
			});

		// While the deferral is armed the harness is a safe stopping point:
		// one unread message is nudged right away, not held.
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(decide(0)).toEqual({ nudge: true, lastNudged: 1 });

		// Had the nudge been refused (say a draft), it would retry. The exit
		// settles the deferral to a plain `waiting`, which stays sendable.
		endSub();
		vi.advanceTimersByTime(61_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(gate()).toEqual({ ok: true });
		expect(decide(0)).toEqual({ nudge: true, lastNudged: 1 });
		harnessActivity.forget(id);
		subagents.forget(id);
	});

	it("without a subagent_end the deferral never settles early: delegating persists past the settle window", () => {
		const id = attached();
		startSub();
		send({ kind: "awaiting_prompt" });
		vi.advanceTimersByTime(61_000);
		// This is the pre-fix field symptom: no exit event, so the working
		// set stays non-empty and the label stays "delegating".
		expect(harnessActivity.get(id)?.phase).toBe("running");
		expect(label(id)).toBe("delegating · 1 agent");
		harnessActivity.forget(id);
		subagents.forget(id);
	});
});
