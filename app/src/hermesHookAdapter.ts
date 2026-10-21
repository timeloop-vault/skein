// #625: the pure half of the Hermes hook adapter (L2c-3). Hermes' shell
// hooks reach the frontend as `skein://harness-hermes-hook` events (one per
// hook firing, see app-tauri `agent_api/hermes_event.rs`); this reducer
// turns each into the phase effects the binding in `harnessEventsHermes.ts`
// applies to `harnessActivity`. No React, no Tauri, no store: state in,
// state + effects out.
//
// What the recorded hooks (CLI and TUI, fixtures in crates/skein-harness)
// establish, and what the rules below lean on:
// - `on_session_start` fires at the FIRST PROMPT, not at launch, and
//   `on_session_end` fires at the end of EVERY turn (not only at exit).
// - A dangerous command emits `pre_approval_request` twice: once with
//   surface `smart` (Hermes' own LLM pre-check, never answered, no dialog)
//   and once with the real dialog (`cli`, or `gateway` in the TUI). Only
//   the real one is paired with a `post_approval_response`.
// - A background `delegate_task` brings a child session whose own
//   start/llm/end hooks arrive carrying the CHILD's session id and platform
//   `subagent`; tool and approval events carry no platform at all, so the
//   child set is tracked from `subagent_start`.
//
// Approval correlation policy (the shell payload has no request id):
// 1. Key = `toolCallId`; without one, `(turnId, description)`. Open
//    approvals are a multiset: two identical keys are two dialogs.
// 2. A response carrying a `toolCallId` removes the open approval with that
//    id, else nothing (a stray response never clears someone else's
//    dialog). One without a `toolCallId` removes the oldest open approval
//    with the same (turnId, description), else the oldest of the same turn
//    (null turn ids match each other), else nothing.
// 3. A main-session turn end clears every open approval; the decisive
//    keystroke clear in harnessActivity still applies on top, because the
//    answer hook can be lost.
// Approvals from a child session DO open permission: the harness is
// blocked on them. Delegation deferral (#628) is out of scope here.
// When the last approval is answered the phase returns to `running` only if
// the main turn is still open (`mainTurnOpen`); a child's approval answered
// after the parent's turn ended has no further hooks coming, so it settles
// to `waiting` instead of stranding on `running`.
//
// Known gap: `childSessions` lives only in memory. After a Skein restart in
// the middle of a delegation it is empty, so a child's `pre_tool_call`
// (which carries no platform) can read as main work and reopen the turn.

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

export interface OpenApproval {
	key: string;
	turnId: string | null;
	toolName: string | null;
	description: string | null;
}

export interface HermesAdapterState {
	/** Session ids of delegated children, learned from `subagent_start`. */
	childSessions: readonly string[];
	/** Dialogs believed open, oldest first. */
	openApprovals: readonly OpenApproval[];
	/** A main-session turn is believed in progress (start seen, no end yet). */
	mainTurnOpen: boolean;
}

export type HermesEffect =
	| { kind: "sessionStarted"; sessionId: string | null; changed: boolean }
	| { kind: "running" }
	| { kind: "waiting"; interrupted: boolean }
	| { kind: "permissionOpen"; toolName: string | null; description: string | null }
	| { kind: "permissionAnswered" }
	| { kind: "permissionAnsweredIdle" }
	| { kind: "turnEnded" };

export interface HermesReduceCtx {
	/** The harness's session id as Skein currently has it. */
	currentSessionId: string | undefined;
}

export const initialHermesState: HermesAdapterState = {
	childSessions: [],
	openApprovals: [],
	mainTurnOpen: false,
};

export const approvalKey = (ev: HermesHookEvent): string =>
	ev.toolCallId !== null
		? `call:${ev.toolCallId}`
		: `turn:${ev.turnId ?? ""}|${ev.description ?? ""}`;

const isChild = (s: HermesAdapterState, ev: HermesHookEvent): boolean =>
	ev.platform === "subagent" || (ev.sessionId !== null && s.childSessions.includes(ev.sessionId));

const none = (state: HermesAdapterState) => ({ state, effects: [] as HermesEffect[] });

/** Index of the open approval a response answers, or -1. */
export function matchApproval(open: readonly OpenApproval[], ev: HermesHookEvent): number {
	if (ev.toolCallId !== null) return open.findIndex((a) => a.key === approvalKey(ev));
	const exact = open.findIndex((a) => a.turnId === ev.turnId && a.description === ev.description);
	if (exact >= 0) return exact;
	return open.findIndex((a) => a.turnId === ev.turnId);
}

const reduceApprovalRequest = (s: HermesAdapterState, ev: HermesHookEvent) => {
	if (ev.surface === "smart") return none(s);
	const open: OpenApproval = {
		key: approvalKey(ev),
		turnId: ev.turnId,
		toolName: ev.toolName,
		description: ev.description,
	};
	return {
		state: { ...s, openApprovals: [...s.openApprovals, open] },
		effects: [
			{ kind: "permissionOpen", toolName: open.toolName, description: open.description },
		] as HermesEffect[],
	};
};

const reduceApprovalResponse = (s: HermesAdapterState, ev: HermesHookEvent) => {
	if (ev.surface === "smart") return none(s);
	const at = matchApproval(s.openApprovals, ev);
	if (at < 0) return none(s);
	const openApprovals = s.openApprovals.filter((_, i) => i !== at);
	const last = openApprovals[openApprovals.length - 1];
	const effect: HermesEffect = last
		? { kind: "permissionOpen", toolName: last.toolName, description: last.description }
		: { kind: s.mainTurnOpen ? "permissionAnswered" : "permissionAnsweredIdle" };
	return { state: { ...s, openApprovals }, effects: [effect] };
};

export function reduceHermesHook(
	s: HermesAdapterState,
	ev: HermesHookEvent,
	ctx: HermesReduceCtx,
): { state: HermesAdapterState; effects: HermesEffect[] } {
	switch (ev.event) {
		case "pre_approval_request":
			return reduceApprovalRequest(s, ev);
		case "post_approval_response":
			return reduceApprovalResponse(s, ev);
		case "subagent_start": {
			const child = ev.childSessionId;
			if (child === null || s.childSessions.includes(child)) return none(s);
			return { state: { ...s, childSessions: [...s.childSessions, child] }, effects: [] };
		}
		// A child's own first hook can beat `subagent_start` here: remember it.
		case "on_session_start":
			if (isChild(s, ev)) {
				const id = ev.sessionId;
				if (id === null || s.childSessions.includes(id)) return none(s);
				return { state: { ...s, childSessions: [...s.childSessions, id] }, effects: [] };
			}
			return {
				state: { ...s, mainTurnOpen: true },
				effects: [
					{
						kind: "sessionStarted",
						sessionId: ev.sessionId,
						changed: ev.sessionId !== null && ev.sessionId !== ctx.currentSessionId,
					},
					{ kind: "running" },
				],
			};
		case "pre_llm_call":
		case "pre_tool_call":
			return isChild(s, ev)
				? none(s)
				: { state: { ...s, mainTurnOpen: true }, effects: [{ kind: "running" }] };
		case "on_session_end":
			if (isChild(s, ev)) return none(s);
			return {
				state: { ...s, openApprovals: [], mainTurnOpen: false },
				effects: [{ kind: "waiting", interrupted: ev.interrupted === true }, { kind: "turnEnded" }],
			};
		// subagent_stop, post_* and anything unknown change no phase.
		default:
			return none(s);
	}
}
