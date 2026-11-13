// #625: the pure half of the Hermes hook adapter (L2c-3). Hermes' shell
// hooks reach the frontend as `skein://harness-hermes-hook` events (one per
// hook firing, see app-tauri `agent_api/hermes_event.rs`). Since #732 the
// daemon's phase machine hears the same hooks and decides every phase,
// permission and delegation transition; this reducer keeps only what the
// webview still acts on: a main-session `on_session_start` (claim and
// capture the session id, attach action extraction) and a main-session
// `on_session_end` (flush the turn's actions). No React, no Tauri, no
// store: state in, state + effects out.
//
// What the recorded hooks (CLI and TUI, fixtures in crates/skein-harness)
// establish, and what the rules below lean on:
// - `on_session_start` fires at the FIRST PROMPT, not at launch, and
//   `on_session_end` fires at the end of EVERY turn (not only at exit).
// - A background `delegate_task` brings a child session whose own
//   start/llm/end hooks arrive carrying the CHILD's session id and platform
//   `subagent`; tool events carry no platform at all, so the child set is
//   tracked from `subagent_start`. A child's session hooks must not claim
//   the harness's session or end its turn.
//
// Known gap: `childSessions` lives only in memory. After a Skein restart in
// the middle of a delegation it is empty, so a child's session hook can
// read as main.

export interface HermesHookEvent {
	roomId: string;
	harnessId: string;
	/** The raw Hermes hook name (`pre_llm_call`, `on_session_end`, …). */
	event: string;
	sessionId: string | null;
	turnId: string | null;
	toolCallId: string | null;
	toolName: string | null;
	platform: string | null;
	surface: string | null;
	description: string | null;
	choice: string | null;
	completed: boolean | null;
	interrupted: boolean | null;
	failed: boolean | null;
	turnExitReason: string | null;
	model: string | null;
	childSessionId: string | null;
}

export interface HermesAdapterState {
	/** Session ids of delegated children, learned from `subagent_start`. */
	childSessions: readonly string[];
}

export type HermesEffect =
	| { kind: "sessionStarted"; sessionId: string | null; changed: boolean }
	| { kind: "turnEnded" };

export interface HermesReduceCtx {
	/** The harness's session id as Skein currently has it. */
	currentSessionId: string | undefined;
}

export const initialHermesState: HermesAdapterState = { childSessions: [] };

const isChild = (s: HermesAdapterState, ev: HermesHookEvent): boolean =>
	ev.platform === "subagent" || (ev.sessionId !== null && s.childSessions.includes(ev.sessionId));

const none = (state: HermesAdapterState) => ({ state, effects: [] as HermesEffect[] });

const addChild = (s: HermesAdapterState, id: string): HermesAdapterState =>
	s.childSessions.includes(id) ? s : { childSessions: [...s.childSessions, id] };

export function reduceHermesHook(
	s: HermesAdapterState,
	ev: HermesHookEvent,
	ctx: HermesReduceCtx,
): { state: HermesAdapterState; effects: HermesEffect[] } {
	switch (ev.event) {
		case "subagent_start":
			// childSessions keeps the id after a stop: the child's late hooks stay filtered.
			return ev.childSessionId === null ? none(s) : none(addChild(s, ev.childSessionId));
		// A child's own first hook can beat `subagent_start` here: remember it.
		case "on_session_start":
			if (isChild(s, ev)) return none(ev.sessionId === null ? s : addChild(s, ev.sessionId));
			return {
				state: s,
				effects: [
					{
						kind: "sessionStarted",
						sessionId: ev.sessionId,
						changed: ev.sessionId !== null && ev.sessionId !== ctx.currentSessionId,
					},
				],
			};
		case "on_session_end":
			return isChild(s, ev) ? none(s) : { state: s, effects: [{ kind: "turnEnded" }] };
		default:
			return none(s);
	}
}
