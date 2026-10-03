// Claude half of the L2c translator (epic #50): subscribes to the Rust
// `ClaudeEvent` channel fed by `harness_events_claude.rs`, marks the
// harness authoritative, and translates each event into a phase call on
// `harnessActivity`. Public entry is `harnessEvents.ts`.
//
// Why keep the policy here and not in Rust: the state machine and
// every consumer of it (status bar, badges, OS notifications) lives
// in the frontend. Translating event → phase in Rust would split the
// policy across the boundary; keeping it here means the Rust adapter
// is a pure "what did Claude write to its log" producer, and the
// "what does it mean for the dot" logic stays next to everything else
// that reads from the store.

import { Channel, invoke } from "@tauri-apps/api/core";
import { backgroundTasks } from "./backgroundTasks.ts";
import { logToRust } from "./frontendLog.ts";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { beginAttach, endAttach, guardChannelHandler } from "./harnessEventsShared.ts";
import { subagents } from "./subagents.ts";
import type { HarnessKind } from "./types.ts";

/// Mirror of the Rust enum. `kind` is the serde tag from
/// `harness_events_claude.rs`'s `ClaudeEvent`. Keep these in lock-step;
/// new event types added on the Rust side fall into the `default`
/// branch of the translator switch and are ignored harmlessly until
/// the policy here catches up.
export type ClaudeEvent =
	| { kind: "assistant_turn" }
	| { kind: "tool_use_start"; name: string }
	| { kind: "tool_use_result" }
	// `task_notification` (#445): the row is a `<task-notification>`
	// Claude Code wrote itself, not a typed prompt.
	| { kind: "user_prompt"; task_notification?: boolean }
	| { kind: "awaiting_prompt" }
	| { kind: "attachment" }
	| { kind: "session_end" }
	// #298 — Claude subagents (Task tool). A subagent transcript
	// appeared, or one was found still running at attach time
	// (`subagent_start`, idempotent per `agent_id` — see
	// `subagents.record`); it reached a terminal stop reason
	// (`subagent_end`); it got a tool result, proof its own permission
	// gate (if any) is gone (`subagent_tool_result`).
	//
	// `initial` (#277): `true` only for the batch `attach_at` seeds
	// from disk at attach time — see the doc comment on the Rust
	// `SubagentStart` variant. Read defensively as `=== true`, same as
	// every other boolean crossing this boundary.
	| {
			kind: "subagent_start";
			agent_id: string;
			agent_type: string | null;
			description: string | null;
			initial: boolean;
	  }
	| { kind: "subagent_tool_result"; agent_id: string }
	| {
			kind: "subagent_end";
			agent_id: string;
			agent_type: string | null;
			description: string | null;
	  }
	// #445 — background Bash/PowerShell/Monitor tasks. `initial` as for
	// `subagent_start`; `agent_id` is the launching subagent, null for
	// the main session.
	| {
			kind: "background_start";
			task_id: string;
			tool_use_id: string;
			task_kind: BackgroundTaskKind;
			description: string | null;
			command: string | null;
			timeout_ms: number | null;
			persistent: boolean;
			auto_backgrounded: boolean;
			agent_id: string | null;
			initial: boolean;
	  }
	| {
			kind: "background_end";
			task_id: string;
			task_kind: BackgroundTaskKind;
			agent_id: string | null;
			status: BackgroundEndStatus;
			exit_code: number | null;
	  };

export type BackgroundTaskKind = "bash" | "powershell" | "monitor";

/// `subagent_ended`: the launching subagent exited, so Skein can't
/// know whether the task finished.
export type BackgroundEndStatus =
	| "completed"
	| "failed"
	| "killed"
	| "stopped"
	| "expired"
	| "task_stopped"
	| "subagent_ended"
	| "unknown";

/// Whether a harness is the kind of thing `attachClaudeEvents` ever
/// attaches to: a Claude harness with a pre-allocated session uuid
/// (chapter 5's `--session-id`). Exported so #410's "Reattach
/// telemetry" action — the harness actions menu item and its command
/// palette twin — can gate on exactly the same condition that decided
/// whether there's a tail to reattach in the first place, rather than
/// a second copy of the kind check that could drift from it.
export function hasClaudeTranscriptTail(
	kind: HarnessKind,
	sessionId: string | undefined,
): sessionId is string {
	return kind === "claude" && typeof sessionId === "string" && sessionId.length > 0;
}

/// Subscribe a Claude harness to its JSONL event stream. Marks the
/// activity store as authoritative-source so the L2a idle tick stops
/// fighting the adapter. Returns an unsubscribe — callers should run
/// it from the same cleanup that kills the PTY.
///
/// Soft-fail: if the Rust side rejects `claude_events_attach` (HOME
/// unset, parent dir unwritable, …) we log and fall back to L2a. The
/// harness keeps working, just without the sharper waiting signal.
export function attachClaudeEvents(
	harnessId: string,
	roomId: string,
	sessionId: string,
	cwd: string,
	// #336: true when the PTY was spawned just now, so a transcript that
	// stops mid-turn belongs to a dead process and Rust replays it as
	// `awaiting_prompt`. False for a re-point under a running process.
	freshProcess = false,
): () => void {
	const channel = new Channel<ClaudeEvent>();
	let closed = false;
	const { token, isLive } = beginAttach(harnessId, { kind: "claude", sessionId, cwd });
	channel.onmessage = guardChannelHandler(harnessId, "claude_events", (event) => {
		// #259: any event at all proves the tail is on the right file.
		// Guarded so a straggler after unsubscribe can't hand authority
		// back to an adapter that no longer exists. #116: unsubscribe is
		// no longer only end-of-life — a `/clear` re-point now detaches
		// and reattaches this same harness mid-life, so a straggler from
		// the OLD tail landing after that detach must not feed the NEW
		// session's phase/subagent state either.
		// #422: a live adapter speaking after `session_end` (the
		// transcript reappeared) takes authority back.
		if (!closed && isLive()) {
			harnessActivity.adapterDelivered(harnessId, {
				restoresAuthority: event.kind !== "session_end",
			});
			translate(harnessId, event);
		}
	});

	// Mark authoritative *synchronously*, not on `.then()`. By the
	// time attachClaudeEvents is called, the PTY has been streaming
	// chunks for some time (LiveTerminal calls us after pty_spawn
	// resolves). If we wait for the rust attach to round-trip
	// before flipping authoritative, those in-flight chunks call
	// recordOutput → setPhase("running") and override whatever the
	// adapter's history probe is about to emit. Net result: harness
	// stays green even though it should be waiting. Found in
	// v0.1.9-dev. Detach in the catch handler if the rust side
	// rejects, so we fall back to L2a cleanly.
	harnessActivity.attachAuthoritativeSource(harnessId);

	logToRust("info", "claude_events", `attach invoking harness=${harnessId} session=${sessionId}`);
	void invoke("claude_events_attach", {
		harnessId,
		roomId,
		sessionId,
		cwd,
		freshProcess,
		onEvent: channel,
	}).catch((err: unknown) => {
		const msg = err instanceof Error ? err.message : String(err);
		console.warn(`[skein] claude_events_attach failed for ${harnessId}:`, msg);
		logToRust(
			"warn",
			"claude_events",
			`attach failed harness=${harnessId} session=${sessionId}: ${msg}`,
		);
		if (isLive()) harnessActivity.detachAuthoritativeSource(harnessId);
	});

	return () => {
		closed = true;
		if (isLive()) harnessActivity.detachAuthoritativeSource(harnessId);
		endAttach(harnessId, token);
		void invoke("claude_events_detach", { harnessId }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn(`[skein] claude_events_detach failed for ${harnessId}:`, msg);
			logToRust(
				"warn",
				"claude_events",
				`detach failed harness=${harnessId} session=${sessionId}: ${msg}`,
			);
		});
	};
}

/// #410: manual recovery for the case the badge is visibly wrong
/// because the Rust-side tail died silently (issue #362's motivating
/// bug). Mirrors the Rust `ClaudeEventsReattachOutcome` enum — keep in
/// lock-step.
///
/// - `"reattached"`: the tail was dead and is live again; the phase
///   settles on its own from the events that follow, same as a fresh
///   attach.
/// - `"healthy"`: nothing to do — logged on the Rust side already.
/// - `"not_attached"`: Rust never had a tail for this harness (e.g.
///   attach never succeeded); only a restart re-attaches it.
export type ReattachOutcome = "reattached" | "healthy" | "not_attached";

/// Invoke wrapper for `claude_events_reattach`, next to
/// `attachClaudeEvents`'s own `claude_events_attach` call. Logs the
/// outcome (or the failure) via `logToRust` so a reattach attempt
/// survives in `skein.log` even from a release build with devtools
/// closed — rejects are re-thrown for the caller to turn into
/// user-facing feedback.
export async function reattachClaudeTelemetry(harnessId: string): Promise<ReattachOutcome> {
	try {
		const outcome = await invoke<ReattachOutcome>("claude_events_reattach", { harnessId });
		logToRust("info", "claude_events", `reattach harness=${harnessId} outcome=${outcome}`);
		return outcome;
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		logToRust("warn", "claude_events", `reattach failed harness=${harnessId}: ${msg}`);
		throw err;
	}
}

const translate = (harnessId: string, event: ClaudeEvent): void => {
	switch (event.kind) {
		case "assistant_turn":
			// Adapter saw the start of a Claude turn. Persisted with
			// `l2c1-claude-assistant` so the activity feed can tell
			// "started thinking" apart from "tool result returned".
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeAssistant);
			return;
		case "tool_use_start":
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeToolUse);
			return;
		case "tool_use_result":
			// #86: a denial writes a tool_result with `is_error` set,
			// same shape as any other result — verified against real
			// Claude Code sessions. Either way the permission gate this
			// tool call was waiting on is gone, approved or not.
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeToolResult, {
				clearsPermission: true,
			});
			return;
		case "user_prompt":
			// #86: the user submitted a fresh prompt — whatever
			// permission dialog might have been showing is gone by
			// construction (Claude doesn't accept a new prompt while
			// one is up).
			harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeUserPrompt, {
				clearsPermission: true,
			});
			// #277: a fresh prompt starts a new delegation-counting
			// window.
			harnessActivity.noteUserPrompt(harnessId);
			return;
		case "awaiting_prompt":
			// The signal we built this for: an assistant row with a
			// terminal stop_reason (end_turn / stop_sequence /
			// max_tokens) → "I'm done, awaiting your next prompt."
			// #260: also an interrupt row or a turn_duration row — an
			// interrupted turn never gets a terminal stop_reason.
			//
			// #277: routed through `awaitingPromptFromAdapter`, not
			// `setWaitingFromAdapter` directly — the main transcript's
			// own end of turn is a lie about the harness being done
			// while subagents it just delegated to are still working.
			harnessActivity.awaitingPromptFromAdapter(harnessId, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
			return;
		case "attachment":
			// User-side action between turns — doesn't shift phase
			// (the harness is still effectively waiting until the
			// user submits). No-op.
			return;
		case "session_end":
			// File vanished. Fall back to L2a — chunk-driven idle
			// detection takes over. The Rust adapter stays alive in
			// case the file reappears, but until then the dot
			// reflects PTY truth. #86: a dialog open at this moment
			// would otherwise pin `permission` for good — the L2a tick
			// never touches a non-running phase, and no tool_result can
			// arrive from a file that is gone. #422: the handler only
			// translates for the live attach; and if the file reappears, the next event
			// restores authority (`adapterDelivered`).
			harnessActivity.releasePermission(harnessId, TRANSITION_SOURCE.AdapterDetached);
			harnessActivity.detachAuthoritativeSource(harnessId);
			return;
		// #298: subagent bookkeeping — none of these three may call
		// `setRunningFromAdapter` or `setWaitingFromAdapter` directly.
		// The parent (main-session) PHASE is deliberately left untouched
		// here: a subagent starting, finishing, or getting a tool result
		// says nothing about whether the main session is running,
		// waiting, or idle, and guessing wrongly would fight whatever
		// the main transcript's own events are already saying. #277
		// adds bookkeeping (`note*`) alongside these that feeds the
		// deferred-end-of-turn timers, but still no direct phase write.
		case "subagent_start":
			subagents.record(
				harnessId,
				{ agentId: event.agent_id, agentType: event.agent_type, description: event.description },
				event.initial,
			);
			// #277: only a LIVE start counts toward `delegatedCount` and
			// counts as proof-of-life for the ceiling — an attach-time
			// replay belongs to a PTY that's already dead (see
			// `SubagentEntry.fromAttach`) and must not look like fresh
			// activity.
			if (event.initial !== true) harnessActivity.noteSubagentStarted(harnessId);
			return;
		case "subagent_end":
			subagents.finish(harnessId, event.agent_id);
			harnessActivity.noteSubagentActivity(harnessId);
			return;
		case "subagent_tool_result":
			// The one narrow phase effect a subagent's own tool result is
			// allowed: prove its own permission gate is gone — not a
			// different subagent's, which `event.agent_id` lets
			// `clearPermission` tell apart. See
			// `harnessActivity.clearPermission`.
			harnessActivity.clearPermission(
				harnessId,
				TRANSITION_SOURCE.L2c1ClaudeSubagentToolResult,
				event.agent_id,
			);
			// #277: proof of life either way, phase effect or not.
			harnessActivity.noteSubagentActivity(harnessId);
			return;
		case "background_start":
			// #446: a background task holds an end of turn like a subagent.
			// No direct phase write; the deferral reads the registry.
			backgroundTasks.record(
				harnessId,
				{
					taskId: event.task_id,
					kind: event.task_kind,
					description: event.description,
					command: event.command,
					timeoutMs: event.timeout_ms,
					persistent: event.persistent,
					agentId: event.agent_id,
				},
				event.initial === true,
			);
			// A live subagent's row is proof of life; an attach replay is not.
			if (event.initial !== true && event.agent_id !== null) {
				harnessActivity.noteSubagentActivity(harnessId);
			}
			return;
		case "background_end":
			backgroundTasks.finish(harnessId, event.task_id);
			if (event.agent_id !== null) harnessActivity.noteSubagentActivity(harnessId);
			return;
	}
};
