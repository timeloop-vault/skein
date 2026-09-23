// Data model for the harness activity state machine (epic #50, L1+L3+L2a):
// the phase enum, the per-harness record shape, and the transition
// provenance vocabulary. Split out of harnessActivity.ts (#19) — pure
// types + one const object, no store, no behaviour. See harnessActivity.ts
// for the module this belongs to.

export type ActivityPhase = "spawning" | "running" | "idle" | "waiting" | "permission" | "exited";

export interface HarnessActivity {
	phase: ActivityPhase;
	/// Epoch ms of the most recent PTY output. `null` until first
	/// chunk arrives — useful for distinguishing "spawning, never
	/// produced output" from "spawned, ran, went idle."
	lastOutputAt: number | null;
	/// Set when `phase === "exited"`. `null` if the PTY exited
	/// without a numeric code (signal-killed on some platforms).
	exitCode: number | null;
	spawnedAt: number;
	/// Has the user typed anything into this harness since it
	/// spawned (paste / arrow keys / any byte that wasn't a
	/// focus-in/-out escape)? Used by L5a notification logic to
	/// tell "user did a task" cycles apart from "startup banner
	/// printed then quieted." Ephemeral; reset on every spawn.
	hasUserInput: boolean;
	/// True when a harness-native adapter (epic #50 L2c) is wired
	/// up for this harness — Claude JSONL tail today, opencode
	/// later. While set, the idle tick stands down and only the
	/// adapter (plus PTY exit) writes phase transitions. The PTY
	/// chunk stream still updates `lastOutputAt` for diagnostics.
	authoritative: boolean;
	/// Rolling tail of stripped PTY output, used by the L2b
	/// pattern-match fallback (copilot / byoh / shell). Only
	/// maintained when `authoritative === false` — the L2c
	/// adapters supersede pattern matching for kinds that have
	/// real event streams. Capped at `TAIL_MAX_CHARS` so it
	/// doesn't grow without bound; the matcher only looks at the
	/// last ~256 chars anyway.
	tail: string;
	/// Which tool triggered the current `permission` phase, when the
	/// adapter can say (Claude's PermissionRequest hook names it;
	/// opencode's permission-asked SSE event doesn't, so `null` there).
	/// Reset on every spawn; cleared automatically whenever the phase
	/// leaves `permission` (see `setPhase`) so a stale name never
	/// survives into whatever the harness does next. #86.
	permissionTool: string | null;
	/// Which subagent raised the current `permission` phase, when the
	/// adapter can say — Claude's PermissionRequest hook fires inside a
	/// subagent transcript same as the main one, and names the
	/// subagent's `agent_type` when it isn't the main session. `null`
	/// for a main-session permission or when the adapter can't say.
	/// Reset on every spawn; cleared automatically whenever the phase
	/// leaves `permission`, same as `permissionTool` (#298).
	permissionAgentType: string | null;
	/// The subagent id (`ClaudeEvent.agent_id`) that raised the current
	/// `permission` phase, when the adapter can say. `null` for a
	/// main-session permission or when the adapter can't say. Distinct
	/// from `permissionAgentType`, which is a display name and not
	/// guaranteed unique across concurrent subagents — `clearPermission`
	/// correlates on this field so one subagent's tool result can't
	/// clear a different subagent's still-open dialog. Reset on every
	/// spawn; cleared automatically whenever the phase leaves
	/// `permission`, same as `permissionAgentType` (#298).
	permissionAgentId: string | null;
	/// Has the L2c adapter delivered at least one event since spawn?
	/// Proof it is reading the right file / stream. #259.
	adapterHeard: boolean;
	/// When the user first pressed Enter while an attached adapter had
	/// still said nothing — arms the silent-adapter watchdog. `null`
	/// until then. #259.
	promptSubmittedAt: number | null;
	/// The watchdog gave up on this harness's adapter and handed phase
	/// back to L2a. Cleared (and authority restored) the moment the
	/// adapter does deliver. #259.
	adapterSilent: boolean;
	/// Which watchdog set `adapterSilent`, so a recovery signal can tell
	/// the two diagnoses apart. `null` whenever `adapterSilent` is
	/// false. Deliberately an explicit field rather than something
	/// inferred (e.g. from `promptSubmittedAt === null`) — the two
	/// degrade helpers currently sit in an `else if`, so today the
	/// inference happens to match, but that's an accident of the guard
	/// order, exactly the kind of coupling that breaks silently the
	/// next time someone edits it. #273.
	degradedBy: "adapter-silent" | "launch-silent" | null;
	/// #273: epoch ms at which the harness's own CLI reported its own
	/// launch — today, Claude's `SessionStart` command hook (wired up by
	/// #215's config injection) POSTing to `/api/harness/session-start`,
	/// re-broadcast on `skein://harness-session-start`. `null` until
	/// that arrives — which is permanent for a spawn with injection off,
	/// or for an authoritative kind whose CLI has no launch hook at all
	/// (opencode today).
	///
	/// Deliberately a SEPARATE field from `adapterHeard`, not folded
	/// into it, even though both mean roughly "this harness proved it's
	/// alive": `adapterHeard` specifically means "the JSONL/SSE tail is
	/// reading the right file/stream" — that is exactly what #259's
	/// silent-adapter watchdog keys on to detect a mis-encoded
	/// transcript path. The launch hook is a different channel
	/// entirely (an HTTP POST from inside the CLI, nothing to do with
	/// the tail Skein reads) and proves nothing about whether the tail
	/// is attached to the right file. Folding this into `adapterHeard`
	/// would make a harness whose CLI reported launch but whose tail is
	/// reading the wrong path look identical to a healthy one — exactly
	/// the failure mode #259 exists to catch — so the two stay apart:
	/// two facts, two fields. Reset to `null` on every spawn.
	launchSignalAt: number | null;
	/// Did #215's config injection actually happen for this spawn (a
	/// non-empty `Injection`, per `pty_spawn`'s resolved `injected`
	/// field)? Reset to `false` on every spawn and set once
	/// `pty_spawn` resolves — the window between the two is "we don't
	/// know yet," which the #238 nudge gate treats the same as "no."
	/// Without #215 there is no proof the harness's CLI even has the
	/// review MCP tools wired up, let alone that a pasted nudge will
	/// reach an agent that can act on it.
	injected: boolean;
	/// #277 (epic #298): epoch ms the main transcript's end of turn
	/// was deferred because `subagents.workingCount` was non-zero at
	/// the time, or `null` when no deferral is armed. Arming changes
	/// no phase — the harness stays whatever it already was (`running`
	/// or `permission`). Disarmed by any main-transcript work signal
	/// (`setRunningFromAdapter`/`setWaitingFromAdapter`/`exited`, plus
	/// a fresh `spawned` record starts with it unset) — see
	/// `awaitingPromptFromAdapter` and the tick's Rules 3/4 for how a
	/// deferral eventually resolves on its own. In-memory only, reset
	/// on every spawn.
	delegationDeferredAt: number | null;
	/// #277: epoch ms of the most recent subagent event of any kind
	/// (start/tool-result/end) — what the Rule 4 ceiling measures
	/// silence against. Bumped by `noteSubagentStarted` (live starts
	/// only) and `noteSubagentActivity`; both mutate silently, the way
	/// `recordOutput` bumps `lastOutputAt`. In-memory only, reset on
	/// every spawn.
	delegationActivityAt: number;
	/// #277: epoch ms the tick first observed the working-subagent set
	/// empty while a deferral was armed, or `null`. What the Rule 3
	/// settle timer measures against. **The tick is the only writer**
	/// — it sets this when it observes empty and clears it if the set
	/// becomes non-empty again — kept that way deliberately: one
	/// writer is what keeps this field debuggable. In-memory only,
	/// reset on every spawn.
	delegationEmptiedAt: number | null;
	/// #277: how many subagents have started live since the user's
	/// last prompt to this harness. Consumed by the notification
	/// wording (not built here — see the issue) for "N delegated
	/// agents finished"; reset by `spawned` and by `noteUserPrompt`.
	/// In-memory only.
	///
	/// Deliberately cumulative across a flush: nothing resets it when
	/// the phase moves to `waiting` (settle, ceiling, or a plain
	/// adapter end-of-turn with no deferral) — only the next prompt
	/// does. That's what makes "3 delegated agents finished" true for
	/// the whole prompt cycle rather than just since the last flush.
	/// A future call site that flips the phase without going through
	/// `noteUserPrompt` would silently reintroduce a stale count.
	delegatedCount: number;
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
/// lifecycle event fired a given phase transition. Persisted to
/// `harness_events.source` and surfaced as the "why" chip in the
/// L7 activity feed. The set is open-ended — new strategies
/// (future L2d, additional adapters) just pick new strings — but
/// the canonical values used today are exposed here so call sites
/// can use the constants instead of stringly-typed literals.
export type TransitionSource = string;

export const TRANSITION_SOURCE = {
	// PTY-driven (L1 substrate).
	PtyOutput: "pty-output",
	PtyExit: "pty-exit",
	// L2a — idle heuristic on the worktree-watcher tick.
	L2aIdle: "l2a-idle",
	// L2b — generic prompt-pattern fallback.
	L2bPattern: "l2b-pattern",
	L2bDrained: "l2b-drained",
	// L2c-1 — Claude JSONL adapter.
	L2c1ClaudeAssistant: "l2c1-claude-assistant",
	L2c1ClaudeEndTurn: "l2c1-claude-end-turn",
	L2c1ClaudeToolUse: "l2c1-claude-tool-use",
	L2c1ClaudeToolResult: "l2c1-claude-tool-result",
	L2c1ClaudeUserPrompt: "l2c1-claude-user-prompt",
	// L2c-2 — opencode SSE adapter.
	L2c2OpencodeBusy: "l2c2-opencode-busy",
	L2c2OpencodeIdle: "l2c2-opencode-idle",
	L2c2OpencodeMessageDelta: "l2c2-opencode-message-delta",
	L2c2OpencodeToolUse: "l2c2-opencode-tool-use",
	// #86 — permission promoted to its own phase.
	L2c1ClaudePermission: "l2c1-claude-permission-request",
	L2c2OpencodePermission: "l2c2-opencode-permission-asked",
	L2c2OpencodePermissionReplied: "l2c2-opencode-permission-replied",
	L2c2OpencodeQuestion: "l2c2-opencode-question-asked",
	L2c2OpencodeQuestionResolved: "l2c2-opencode-question-resolved",
	// The only "answered" signal Claude gives us for its own dialog:
	// the user typed something decisive into the PTY. See
	// `isDecisiveInput`.
	UserInputPermission: "user-input-permission",
	// #86: the adapter that could have resolved a permission dialog
	// went away. See `releasePermission`.
	AdapterDetached: "adapter-detached",
	// #259: the adapter never delivered anything after a prompt was
	// submitted — the watchdog handed the harness back to L2a.
	AdapterSilent: "adapter-silent",
	// #273: no launch signal ever arrived (and the tail hadn't spoken
	// either) within LAUNCH_SILENT_AFTER_MS of spawn — the watchdog
	// handed the harness back to L2a. Deliberately its own source
	// rather than reusing `AdapterSilent`: the two are diagnosed
	// differently (no prompt was even needed to arm this one), and this
	// table exists so the L7 feed's "why" chip says which.
	LaunchSilent: "launch-silent",
	// #273: the harness's own CLI reported its own launch — Claude's
	// `SessionStart` hook via #215 injection. See `noteLaunchSignal`.
	L2c1ClaudeSessionStart: "l2c1-claude-session-start",
	// #116: `/clear` starts a brand new session id mid-process — see
	// `sessionSwitched`.
	L2c1ClaudeSessionClear: "l2c1-claude-session-clear",
	// #116: an in-tool `/resume` (or a pre-v2.1.214 fork, which reported
	// itself the same way) re-points onto a different session id — see
	// `sessionSwitched`.
	L2c1ClaudeSessionResume: "l2c1-claude-session-resume",
	// #116: a `/branch`, `/fork`, `--fork-session` or desktop rewind
	// reports its own dedicated `source: "fork"` from Claude Code
	// v2.1.214 on, carrying the newly forked session id — see
	// `sessionSwitched`.
	L2c1ClaudeSessionFork: "l2c1-claude-session-fork",
	// #298: a subagent's own tool result proves the gate its permission
	// prompt held is gone, even though the main transcript never sees
	// it. See `clearPermission`.
	L2c1ClaudeSubagentToolResult: "l2c1-claude-subagent-tool-result",
	// #277: a deferred end-of-turn resolving on its own, via the tick's
	// Rule 3 (working set went empty and stayed empty) or Rule 4
	// (ceiling — a subagent signal was presumed lost). Deliberately
	// NOT prefixed `l2c1-claude-`: the deferral mechanism is
	// harness-agnostic, keyed only on `subagents.workingCount`, so if
	// opencode ever grows a child-session liveness signal feeding the
	// same registry, it gets this behaviour — and these source names —
	// for free.
	DelegationSettled: "delegation-settled",
	DelegationCeiling: "delegation-ceiling",
} as const;
