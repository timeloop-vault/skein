// Claude half of the L2c adapter plumbing (epic #50): subscribes to the Rust
// `ClaudeEvent` channel fed by `harness_events_claude.rs`. Since #732 the
// phase machine lives in the daemon, which tees this channel itself; what
// the webview still does with it is record the running CLI version
// (`claudeVersionStore`). Public entry is `harnessEvents.ts`.

import { Channel, invoke } from "@tauri-apps/api/core";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import { logToRust } from "./frontendLog.ts";
import { guardChannelHandler } from "./harnessEventsShared.ts";
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
	// #497: the sidecar was unreadable at `subagent_start` and has since
	// supplied a label. Not a start — carries no work signal.
	| {
			kind: "subagent_label";
			agent_id: string;
			agent_type: string | null;
			description: string | null;
	  }
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
	  }
	// #491 — the Claude Code version that wrote a main-transcript row;
	// `timestamp_ms` is the row's own time (null if it had none).
	| { kind: "cli_version"; version: string; timestamp_ms: number | null };

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

/// Subscribe a Claude harness to its JSONL event stream. Returns an
/// unsubscribe — callers should run it from the same cleanup that kills
/// the PTY.
///
/// Soft-fail: if the Rust side rejects `claude_events_attach` (HOME
/// unset, parent dir unwritable, …) we log; the harness keeps working,
/// just without the sharper waiting signal.
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
	channel.onmessage = guardChannelHandler(harnessId, "claude_events", (event) => {
		// #116: a `/clear` re-point detaches and reattaches this same
		// harness mid-life, so a straggler from the OLD tail landing after
		// that detach must not feed the NEW session's state.
		if (!closed) translate(harnessId, event);
	});

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
	});

	return () => {
		closed = true;
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

/// Since #732 the daemon tees this channel into its phase machine, so the
/// only event with a client-side effect is the CLI version.
const translate = (harnessId: string, event: ClaudeEvent): void => {
	if (event.kind === "cli_version") {
		claudeVersionStore.recordRunning(harnessId, event.version, event.timestamp_ms);
	}
};
