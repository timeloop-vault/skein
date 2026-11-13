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
describe("plain turn (A)", () => {
	for (const f of ["hooks-A.jsonl", "tui-hooks-A.jsonl"]) {
		it(`${f}: start -> sessionStarted, end -> turnEnded`, () => {
			const { kinds } = run(fixture(f));
			expect(kinds).toEqual(["sessionStarted", "turnEnded"]);
		});
	}
});

describe("approvals, interrupts (B, C)", () => {
	for (const f of ["hooks-B.jsonl", "tui-hooks-B.jsonl", "hooks-C.jsonl", "tui-hooks-C.jsonl"]) {
		it(`${f}: the daemon owns phases; only the session effects remain`, () => {
			const { kinds } = run(fixture(f));
			expect(kinds.filter((k) => k !== "sessionStarted" && k !== "turnEnded")).toEqual([]);
			expect(kinds[0]).toBe("sessionStarted");
			expect(kinds[kinds.length - 1]).toBe("turnEnded");
		});
	}
});

describe("background delegation (D)", () => {
	it("child events produce no effects; the parent's end flushes the turn", () => {
		const events = fixture("hooks-D.jsonl");
		const parent = events[0]?.sessionId;
		const { steps, state } = run(events);
		expect(state.childSessions.length).toBe(1);
		for (const s of steps) {
			if (s.ev.sessionId !== parent && s.ev.sessionId !== null) {
				expect(s.effects).toEqual([]);
			}
		}
		const ends = steps.filter((s) => s.ev.event === "on_session_end" && s.ev.sessionId === parent);
		expect(ends.length).toBeGreaterThan(0);
		for (const e of ends) expect(e.effects.map((x) => x.kind)).toEqual(["turnEnded"]);
	});

	it("a child's first hook before subagent_start is still treated as a child", () => {
		const out = run([ev({ event: "on_session_start", sessionId: "C", platform: "subagent" })]);
		expect(out.kinds).toEqual([]);
		expect(out.state.childSessions).toEqual(["C"]);
		expect(run([ev({ event: "on_session_end", sessionId: "C" })], out.state).kinds).toEqual([]);
	});

	it("subagent_start remembers the child so its late hooks stay filtered", () => {
		const s0 = run([ev({ event: "subagent_start", childSessionId: "C" })]).state;
		expect(s0.childSessions).toEqual(["C"]);
		const stopped = run([ev({ event: "subagent_stop", childSessionId: "C" })], s0).state;
		expect(run([ev({ event: "on_session_start", sessionId: "C" })], stopped).kinds).toEqual([]);
		expect(run([ev({ event: "subagent_start" })]).state.childSessions).toEqual([]);
	});
});

describe("session start", () => {
	it("reports whether the id changed", () => {
		const e = ev({ event: "on_session_start", sessionId: "S2" });
		const same = reduceHermesHook(initialHermesState, e, { currentSessionId: "S2" });
		expect(same.effects[0]).toEqual({ kind: "sessionStarted", sessionId: "S2", changed: false });
		const diff = reduceHermesHook(initialHermesState, e, { currentSessionId: "S1" });
		expect(diff.effects[0]).toEqual({ kind: "sessionStarted", sessionId: "S2", changed: true });
	});

	it("a hook with no session effect (pre_tool_call, approvals) does nothing here", () => {
		const kinds = run([
			ev({ event: "pre_tool_call" }),
			ev({ event: "pre_approval_request", surface: "cli" }),
			ev({ event: "post_approval_response", surface: "cli" }),
		]).kinds;
		expect(kinds).toEqual([]);
	});
});
