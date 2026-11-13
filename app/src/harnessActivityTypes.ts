// Data model for the harness activity view (epic #50): the phase enum,
// the per-harness view shape and the transition provenance vocabulary.
// Since #732 the daemon's phase machine owns the record and the webview holds
// a mirror of its `PhaseView` (`phaseMirror.ts`); these are the shapes it
// deserializes into. Pure types + one const object, no store, no behaviour.

export type ActivityPhase = "spawning" | "running" | "idle" | "waiting" | "permission" | "exited";

/// Mirror of `PhaseView` in skein-daemon `harness_phase/stream.rs`: the part
/// of the daemon's phase record that crosses the wire. The machine's own
/// record never does. Every timestamp is epoch ms.
export interface HarnessActivity {
	// Display and notifications.
	phase: ActivityPhase;
	/// Epoch ms of the last REAL phase change; spawn time until the first.
	phaseSince: number;
	spawnedAt: number;
	/// Has the user typed anything into this harness since it spawned (paste
	/// / arrow keys / any byte that wasn't a focus-in/-out escape)? Tells
	/// "user did a task" cycles apart from "startup banner printed then
	/// quieted" for notifications. Reset on every spawn.
	hasUserInput: boolean;
	/// Which tool triggered the current `permission` phase, when the adapter
	/// can say (Claude's PermissionRequest hook names it; opencode doesn't).
	/// The daemon clears it whenever the phase leaves `permission`. #86.
	permissionTool: string | null;
	/// Which subagent raised the current `permission` phase (its `agent_type`),
	/// when the adapter can say; `null` for a main-session dialog (#298).
	permissionAgentType: string | null;
	/// #277: how many subagents have started live since the user's last
	/// prompt, for the "N delegated agents finished" wording. Cumulative
	/// across a flush; reset by the next prompt.
	delegatedCount: number;
	/// #423: the main session's latest adapter turn signal, "work" or "end",
	/// whatever the daemon then did with it. `null` until the first.
	lastTurnSignal: { kind: "work" | "end"; at: number } | null;
	/// #423: epoch ms of the user's last Enter/submit in this PTY. `null`
	/// until the first.
	lastSubmitAt: number | null;

	// The send gate: `canSendPrompt` and mail delivery still decide here. This
	// whole group leaves the wire with #733, which moves those decisions into
	// the daemon.

	/// True when a harness-native adapter (epic #50 L2c) is wired up. While
	/// set, only the adapter (plus PTY exit) writes phase transitions.
	authoritative: boolean;
	/// Has the L2c adapter delivered at least one event since spawn? Proof it
	/// is reading the right file / stream. #259.
	adapterHeard: boolean;
	/// The watchdog gave up on this harness's adapter and handed phase back
	/// to L2a. Cleared the moment the adapter does deliver. #259.
	adapterSilent: boolean;
	/// #273: epoch ms at which the harness's own CLI reported its launch
	/// (Claude's `SessionStart` hook). `null` until then; permanent for a
	/// spawn with injection off. A separate fact from `adapterHeard`: that
	/// one proves the transcript tail is reading the right file, this one
	/// does not.
	launchSignalAt: number | null;
	/// Epoch ms of the most recent PTY output, `null` until the first chunk.
	/// The daemon throttles it; the mirror keeps a newer local stamp.
	lastOutputAt: number | null;
	/// An opencode `question` prompt is outstanding (#672): the phase reads
	/// `waiting` but the turn has not ended, so `canSendPrompt` refuses.
	questionOpen: boolean;
	/// #277: epoch ms the main transcript's end of turn was deferred because
	/// subagents or background tasks were still working, or `null` when no
	/// deferral is armed. Arming changes no phase.
	delegationDeferredAt: number | null;
	/// Did #215's config injection happen for this spawn? Without it there
	/// is no proof the CLI has the review tools wired up, so the #238 nudge
	/// gate refuses.
	injected: boolean;
}

/// Global transition callback: receives every real phase change
/// plus the `source` string identifying which strategy fired it
/// (`l2a-idle`, `l2b-pattern`, `l2c1-claude-end-turn`, …).
///
/// Used by App-level notification logic (#12 L5a tab badges, L5b
/// OS notifications, L5c toasts) which need to react to transitions
/// across every harness, and by the L6 sqlite writer that persists
/// each transition with its provenance for the L7 activity feed.
export type TransitionListener = (
	id: string,
	from: ActivityPhase,
	to: ActivityPhase,
	source: TransitionSource,
) => void;

/// Free-form string identifying which detection strategy or
/// lifecycle event fired a given phase transition, as the daemon reports
/// it (it persists the row to `harness_events.source`). The set is
/// open-ended; the values the webview itself branches on are exposed here
/// so call sites can use the constants instead of stringly-typed literals.
export type TransitionSource = string;

export const TRANSITION_SOURCE = {
	// #731: a view re-attached to a process that was already running; not news.
	Reattached: "reattached",
	L2c2OpencodeIdle: "l2c2-opencode-idle",
	// #175: assumed-idle baseline after an SSE (re)connect — a guess,
	// never notify-worthy (see `isNotifiable`).
	L2c2OpencodeBaseline: "l2c2-opencode-baseline",
	// #277: a deferred end-of-turn resolving on its own, via the tick's
	// Rule 3 (working set went empty and stayed empty) or Rule 4
	// (ceiling — a signal was presumed lost, or a task held too long). Deliberately
	// NOT prefixed `l2c1-claude-`: the deferral mechanism is
	// harness-agnostic, keyed on outstanding work (`subagents` plus
	// `backgroundTasks` working counts, #446).
	DelegationSettled: "delegation-settled",
	DelegationCeiling: "delegation-ceiling",
	// #441: the stale-work watchdog. A `running` authoritative harness
	// with outstanding work but no deferral armed and no signal for the
	// ceiling is presumed to have lost its end; nothing is claimed
	// finished.
	WorkWatchdog: "work-watchdog",
	// #490: the user (or an agent on their behalf) restarted the harness;
	// the daemon logs the row when it hears `restartRequested` (#732).
	UserRestart: "user-restart",
	// #491: the same restart, started by the Claude Code update notice's
	// auto mode rather than by a person.
	VersionAutoRestart: "version-auto-restart",
} as const;
