import { describe, expect, it } from "vitest";
import cliA from "../../crates/skein-harness/src/hermes/fixtures/hooks-A.jsonl?raw";
import cliB from "../../crates/skein-harness/src/hermes/fixtures/hooks-B.jsonl?raw";
import cliC from "../../crates/skein-harness/src/hermes/fixtures/hooks-C.jsonl?raw";
import cliD from "../../crates/skein-harness/src/hermes/fixtures/hooks-D.jsonl?raw";
import tuiA from "../../crates/skein-harness/src/hermes/fixtures/tui-hooks-A.jsonl?raw";
import tuiB from "../../crates/skein-harness/src/hermes/fixtures/tui-hooks-B.jsonl?raw";
import tuiC from "../../crates/skein-harness/src/hermes/fixtures/tui-hooks-C.jsonl?raw";
import {
	type HermesAdapterState,
	type HermesEffect,
	type HermesHookEvent,
	initialHermesState,
	reduceHermesHook,
} from "./hermesHookAdapter.ts";

type Raw = {
	hook_event_name: string;
	session_id?: string;
	tool_name?: string | null;
	extra?: Record<string, unknown>;
};
const str = (v: unknown) => (typeof v === "string" ? v : null);
const bool = (v: unknown) => (typeof v === "boolean" ? v : null);

/// Maps a recorded snake_case payload to the camelCase event, as hermes_hooks.rs does.
function toEvent(raw: Raw): HermesHookEvent {
	const x = raw.extra ?? {};
	return {
		roomId: "r",
		harnessId: "h",
		event: raw.hook_event_name,
		sessionId: str(raw.session_id),
		turnId: str(x.turn_id),
		toolCallId: str(x.tool_call_id),
		toolName: str(raw.tool_name),
		platform: str(x.platform),
		surface: str(x.surface),
		description: str(x.description),
		choice: str(x.choice),
		completed: bool(x.completed),
		interrupted: bool(x.interrupted),
		failed: bool(x.failed),
		turnExitReason: str(x.turn_exit_reason),
		model: str(x.model),
		childSessionId: str(x.child_session_id),
	};
}

const FILES: Record<string, string> = {
	"hooks-A.jsonl": cliA,
	"hooks-B.jsonl": cliB,
	"hooks-C.jsonl": cliC,
	"hooks-D.jsonl": cliD,
	"tui-hooks-A.jsonl": tuiA,
	"tui-hooks-B.jsonl": tuiB,
	"tui-hooks-C.jsonl": tuiC,
};

const fixture = (name: string): HermesHookEvent[] =>
	(FILES[name] ?? "")
		.split("\n")
		.filter(Boolean)
		.map((l) => toEvent(JSON.parse(l) as Raw));

/// Feed events; returns per-event effects plus the final state.
function run(events: HermesHookEvent[], start: HermesAdapterState = initialHermesState) {
	let state = start;
	const steps: { ev: HermesHookEvent; effects: HermesEffect[] }[] = [];
	for (const e of events) {
		const out = reduceHermesHook(state, e, { currentSessionId: undefined });
		state = out.state;
		steps.push({ ev: e, effects: out.effects });
	}
	return { state, steps, kinds: steps.flatMap((s) => s.effects.map((e) => e.kind)) };
}

const ev = (over: Partial<HermesHookEvent>): HermesHookEvent => ({
	...toEvent({ hook_event_name: "pre_llm_call", session_id: "S" }),
	...over,
});
const req = (over: Partial<HermesHookEvent> = {}) =>
	ev({ event: "pre_approval_request", surface: "cli", turnId: "T", description: "rm", ...over });
const inTurn: HermesAdapterState = { ...initialHermesState, mainTurnOpen: true };
const resp = (over: Partial<HermesHookEvent> = {}) =>
	ev({ event: "post_approval_response", surface: "cli", turnId: "T", description: "rm", ...over });

describe("plain turn (A)", () => {
	for (const f of ["hooks-A.jsonl", "tui-hooks-A.jsonl"]) {
		it(`${f}: start -> running, end -> waiting + turnEnded`, () => {
			const { kinds, state } = run(fixture(f));
			expect(kinds).toEqual(["sessionStarted", "running", "running", "waiting", "turnEnded"]);
			expect(state.openApprovals).toEqual([]);
		});
	}
});

describe("approvals (B)", () => {
	for (const f of ["hooks-B.jsonl", "tui-hooks-B.jsonl"]) {
		it(`${f}: smart pre-check ignored, real dialog opens and closes`, () => {
			const events = fixture(f);
			const { steps, state } = run(events);
			const real = events.filter(
				(e) => e.event === "pre_approval_request" && e.surface !== "smart",
			);
			const opens = steps.filter((s) => s.effects.some((e) => e.kind === "permissionOpen"));
			expect(opens.length).toBe(real.length);
			for (const s of steps.filter((x) => x.ev.surface === "smart")) {
				expect(s.effects).toEqual([]);
			}
			const answered = steps.filter((s) => s.effects.some((e) => e.kind === "permissionAnswered"));
			expect(answered.length).toBe(opens.length);
			expect(state.openApprovals).toEqual([]);
			const kinds = steps.flatMap((s) => s.effects.map((e) => e.kind));
			expect(kinds.indexOf("permissionOpen")).toBeLessThan(kinds.indexOf("permissionAnswered"));
		});
	}

	it("a tool call that follows the dialog never reopens or clears it", () => {
		const out = run([req({ toolCallId: "c1" }), ev({ event: "pre_tool_call" })]);
		expect(out.state.openApprovals.length).toBe(1);
	});
});

describe("interrupt (C)", () => {
	for (const f of ["hooks-C.jsonl", "tui-hooks-C.jsonl"]) {
		it(`${f}: ends the turn as interrupted`, () => {
			const { steps } = run(fixture(f));
			const end = steps.find((s) => s.ev.event === "on_session_end");
			expect(end?.effects).toContainEqual({ kind: "waiting", interrupted: true });
		});
	}
});

describe("background delegation (D)", () => {
	it("child events do not flip the parent; the parent end waits", () => {
		const events = fixture("hooks-D.jsonl");
		const parent = events[0]?.sessionId;
		const { steps, state } = run(events);
		expect(state.childSessions.length).toBe(1);
		for (const s of steps) {
			if (s.ev.sessionId !== parent && s.ev.sessionId !== null) {
				expect(s.effects).toEqual([]);
			}
		}
		const stop = steps.find((s) => s.ev.event === "subagent_stop");
		expect(stop?.effects).toEqual([]);
		const ends = steps.filter((s) => s.ev.event === "on_session_end" && s.ev.sessionId === parent);
		expect(ends.length).toBeGreaterThan(0);
		for (const e of ends) expect(e.effects.map((x) => x.kind)).toEqual(["waiting", "turnEnded"]);
	});

	it("a child's first hook before subagent_start is still treated as a child", () => {
		const out = run([ev({ event: "on_session_start", sessionId: "C", platform: "subagent" })]);
		expect(out.kinds).toEqual([]);
		expect(out.state.childSessions).toEqual(["C"]);
		expect(run([ev({ event: "pre_llm_call", sessionId: "C" })], out.state).kinds).toEqual([]);
	});

	it("a child's approval opens permission", () => {
		const s0 = run([ev({ event: "subagent_start", childSessionId: "C" })]).state;
		const out = run([req({ sessionId: "C", toolCallId: "k", toolName: "terminal" })], s0);
		expect(out.steps[0]?.effects).toEqual([
			{ kind: "permissionOpen", toolName: "terminal", description: "rm" },
		]);
	});
});

describe("main turn tracking", () => {
	it("tracks open and end of the main turn; a child never touches it", () => {
		const open = run([ev({ event: "on_session_start" })]).state;
		expect(open.mainTurnOpen).toBe(true);
		const child = run(
			[ev({ event: "on_session_end", sessionId: "C", platform: "subagent" })],
			open,
		).state;
		expect(child.mainTurnOpen).toBe(true);
		expect(run([ev({ event: "on_session_end" })], open).state.mainTurnOpen).toBe(false);
		expect(run([ev({ event: "pre_tool_call" })]).state.mainTurnOpen).toBe(true);
		expect(run([ev({ event: "pre_llm_call" })]).state.mainTurnOpen).toBe(true);
	});

	it("an answer after the main turn ended settles to waiting, not running", () => {
		const out = run([
			ev({ event: "on_session_start" }),
			ev({ event: "subagent_start", childSessionId: "C" }),
			ev({ event: "on_session_end" }),
			req({ sessionId: "C", toolCallId: "k" }),
			resp({ sessionId: "C", toolCallId: "k" }),
		]);
		expect(out.steps[4]?.effects).toEqual([{ kind: "permissionAnsweredIdle" }]);
	});

	it("an answer during an open turn returns to running", () => {
		const out = run([
			ev({ event: "on_session_start" }),
			req({ toolCallId: "k" }),
			resp({ toolCallId: "k" }),
		]);
		expect(out.steps[2]?.effects).toEqual([{ kind: "permissionAnswered" }]);
	});
});

describe("ambiguity policy", () => {
	it("two same-description approvals without toolCallId are a multiset", () => {
		const two = run([req(), req()], inTurn);
		expect(two.state.openApprovals.length).toBe(2);
		const one = run([resp()], two.state);
		expect(one.state.openApprovals.length).toBe(1);
		expect(one.steps[0]?.effects[0]?.kind).toBe("permissionOpen");
		const none = run([resp()], one.state);
		expect(none.kinds).toEqual(["permissionAnswered"]);
		expect(none.state.openApprovals).toEqual([]);
	});

	it("with toolCallId the matching call is removed, not the oldest", () => {
		const open = run([req({ toolCallId: "a" }), req({ toolCallId: "b" })]).state;
		const out = run([resp({ toolCallId: "b" })], open);
		expect(out.state.openApprovals.map((a) => a.key)).toEqual(["call:a"]);
	});

	it("a post whose toolCallId matches nothing clears nothing", () => {
		const open = run([req({ toolCallId: "a" }), req({ toolCallId: "b" })]).state;
		const out = run([resp({ toolCallId: "zzz" })], open);
		expect(out.state).toEqual(open);
		expect(out.kinds).toEqual([]);
	});

	it("without a toolCallId, (turnId, description) wins over the oldest of the turn", () => {
		const open = run([
			req({ toolCallId: "a", description: "ls" }),
			req({ toolCallId: "b", description: "rm" }),
		]).state;
		const out = run([resp({ description: "rm" })], open);
		expect(out.state.openApprovals.map((a) => a.key)).toEqual(["call:a"]);
	});

	it("without a toolCallId or a description match, the oldest of the turn goes", () => {
		const open = run([
			req({ toolCallId: "a", description: "ls" }),
			req({ toolCallId: "b", description: "cat" }),
		]).state;
		const out = run([resp({ description: "other" })], open);
		expect(out.state.openApprovals.map((a) => a.key)).toEqual(["call:b"]);
	});

	it("a post in another turn, or with nothing open, does nothing", () => {
		const open = run([req({ toolCallId: "a" })]).state;
		expect(run([resp({ turnId: "OTHER" })], open).state).toEqual(open);
		expect(run([resp({ toolCallId: "z", turnId: "OTHER" })], open).state).toEqual(open);
		expect(run([resp()]).kinds).toEqual([]);
	});

	it("null turn ids on both sides match each other", () => {
		const open = run([req({ turnId: null, description: null })]).state;
		expect(run([resp({ turnId: null, description: null })], open).state.openApprovals).toEqual([]);
		const loose = run([req({ turnId: null, description: "x" })]).state;
		expect(run([resp({ turnId: null, description: null })], loose).state.openApprovals).toEqual([]);
	});

	it("turn end clears every open approval", () => {
		const open = run([req(), req({ toolCallId: "x" })]).state;
		const out = run([ev({ event: "on_session_end" })], open);
		expect(out.state.openApprovals).toEqual([]);
		expect(out.kinds).toEqual(["waiting", "turnEnded"]);
	});

	it("session start reports whether the id changed", () => {
		const e = ev({ event: "on_session_start", sessionId: "S2" });
		const same = reduceHermesHook(initialHermesState, e, { currentSessionId: "S2" });
		expect(same.effects[0]).toEqual({ kind: "sessionStarted", sessionId: "S2", changed: false });
		const diff = reduceHermesHook(initialHermesState, e, { currentSessionId: "S1" });
		expect(diff.effects[0]).toEqual({ kind: "sessionStarted", sessionId: "S2", changed: true });
	});
});
