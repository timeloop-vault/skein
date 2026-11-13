//! Data model for the harness phase machine (epic #50, L1+L3+L2a): the
//! phase enum, the per-harness record shape and the transition provenance
//! vocabulary. Port of `app/src/harnessActivityTypes.ts`. Pure types, no
//! store, no behaviour beyond the string mappings.
//!
//! The strings of [`ActivityPhase`] and [`TransitionSource`] are persisted
//! to `harness_events.from_phase/to_phase/source` and the webview mirrors
//! them from the phase stream, so `as_str()` and the serde names must stay
//! byte-identical to the TS values.

// The doc comments are the ported spec and quote TS identifiers verbatim.
#![allow(clippy::doc_markdown)]

use serde::{Deserialize, Serialize};

/// Where a harness is in its life. `permission` outranks `waiting` everywhere (#86).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityPhase {
    Spawning,
    Running,
    Idle,
    Waiting,
    Permission,
    Exited,
}

impl ActivityPhase {
    /// The exact TS string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spawning => "spawning",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Waiting => "waiting",
            Self::Permission => "permission",
            Self::Exited => "exited",
        }
    }
}

/// Which watchdog set `adapter_silent` (#273). See [`HarnessActivity::degraded_by`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DegradedBy {
    #[serde(rename = "adapter-silent")]
    AdapterSilent,
    #[serde(rename = "launch-silent")]
    LaunchSilent,
}

/// The kind of the main session's latest adapter turn signal (#423).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnSignalKind {
    Work,
    End,
}

/// `HarnessActivity::last_turn_signal` (#423).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSignal {
    pub kind: TurnSignalKind,
    pub at: i64,
}

/// `HarnessActivity::last_adapter_event` (#423).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterEvent {
    pub at: i64,
    pub restores_authority: bool,
}

/// The per-harness record (`HarnessActivity` in TS). 28 fields; every
/// timestamp is epoch ms.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct HarnessActivity {
    pub phase: ActivityPhase,
    /// Epoch ms of the most recent PTY output. `None` until first
    /// chunk arrives: useful for distinguishing "spawning, never
    /// produced output" from "spawned, ran, went idle."
    pub last_output_at: Option<i64>,
    /// Set when `phase == Exited`. `None` if the PTY exited
    /// without a numeric code (signal-killed on some platforms).
    pub exit_code: Option<i32>,
    pub spawned_at: i64,
    /// Has the user typed anything into this harness since it
    /// spawned (paste / arrow keys / any byte that wasn't a
    /// focus-in/-out escape)? Used by L5a notification logic to
    /// tell "user did a task" cycles apart from "startup banner
    /// printed then quieted." Ephemeral; reset on every spawn.
    pub has_user_input: bool,
    /// True when a harness-native adapter (epic #50 L2c) is wired
    /// up for this harness (Claude JSONL tail today, opencode
    /// later). While set, the idle tick stands down and only the
    /// adapter (plus PTY exit) writes phase transitions. The PTY
    /// chunk stream still updates `last_output_at` for diagnostics.
    pub authoritative: bool,
    /// Rolling tail of stripped PTY output, used by the L2b
    /// pattern-match fallback (copilot / byoh / shell). Only
    /// maintained when `authoritative == false`: the L2c
    /// adapters supersede pattern matching for kinds that have
    /// real event streams. Capped at `TAIL_MAX_CHARS` so it
    /// doesn't grow without bound; the matcher only looks at the
    /// last ~256 chars anyway.
    pub tail: String,
    /// Which tool triggered the current `permission` phase, when the
    /// adapter can say (Claude's PermissionRequest hook names it;
    /// opencode's permission-asked SSE event doesn't, so `None` there).
    /// Reset on every spawn; cleared automatically whenever the phase
    /// leaves `permission` (see `set_phase`) so a stale name never
    /// survives into whatever the harness does next. #86.
    pub permission_tool: Option<String>,
    /// Which subagent raised the current `permission` phase, when the
    /// adapter can say: Claude's PermissionRequest hook fires inside a
    /// subagent transcript same as the main one, and names the
    /// subagent's `agent_type` when it isn't the main session. `None`
    /// for a main-session permission or when the adapter can't say.
    /// Reset on every spawn; cleared automatically whenever the phase
    /// leaves `permission`, same as `permission_tool` (#298).
    pub permission_agent_type: Option<String>,
    /// The subagent id (`ClaudeEvent.agent_id`) that raised the current
    /// `permission` phase, when the adapter can say. `None` for a
    /// main-session permission or when the adapter can't say. Distinct
    /// from `permission_agent_type`, which is a display name and not
    /// guaranteed unique across concurrent subagents: `clear_permission`
    /// correlates on this field so one subagent's tool result can't
    /// clear a different subagent's still-open dialog. Reset on every
    /// spawn; cleared automatically whenever the phase leaves
    /// `permission`, same as `permission_agent_type` (#298).
    pub permission_agent_id: Option<String>,
    /// An opencode `question` prompt is outstanding (#672). The phase
    /// reads `waiting` (the user's turn) but the turn has not ended, so
    /// `canSendPrompt` refuses while this is set. Not a phase: never
    /// logged as a transition. Reset on every spawn.
    pub question_open: bool,
    /// Has the L2c adapter delivered at least one event since spawn?
    /// Proof it is reading the right file / stream. #259.
    pub adapter_heard: bool,
    /// When the user first pressed Enter while an attached adapter had
    /// still said nothing: arms the silent-adapter watchdog. `None`
    /// until then. #259.
    pub prompt_submitted_at: Option<i64>,
    /// The watchdog gave up on this harness's adapter and handed phase
    /// back to L2a. Cleared (and authority restored) the moment the
    /// adapter does deliver. #259.
    pub adapter_silent: bool,
    /// Which watchdog set `adapter_silent`, so a recovery signal can tell
    /// the two diagnoses apart. `None` whenever `adapter_silent` is
    /// false. Deliberately an explicit field rather than something
    /// inferred (e.g. from `prompt_submitted_at == None`): the two
    /// degrade helpers currently sit in an `else if`, so today the
    /// inference happens to match, but that's an accident of the guard
    /// order, exactly the kind of coupling that breaks silently the
    /// next time someone edits it. #273.
    pub degraded_by: Option<DegradedBy>,
    /// #273: epoch ms at which the harness's own CLI reported its own
    /// launch: today, Claude's `SessionStart` command hook (wired up by
    /// #215's config injection) POSTing to `/api/harness/session-start`,
    /// re-broadcast on `skein://harness-session-start`. `None` until
    /// that arrives, which is permanent for a spawn with injection off,
    /// or for an authoritative kind whose CLI has no launch hook at all
    /// (opencode today).
    ///
    /// Deliberately a SEPARATE field from `adapter_heard`, not folded
    /// into it, even though both mean roughly "this harness proved it's
    /// alive": `adapter_heard` specifically means "the JSONL/SSE tail is
    /// reading the right file/stream", which is exactly what #259's
    /// silent-adapter watchdog keys on to detect a mis-encoded
    /// transcript path. The launch hook is a different channel
    /// entirely (an HTTP POST from inside the CLI, nothing to do with
    /// the tail Skein reads) and proves nothing about whether the tail
    /// is attached to the right file. Folding this into `adapter_heard`
    /// would make a harness whose CLI reported launch but whose tail is
    /// reading the wrong path look identical to a healthy one, exactly
    /// the failure mode #259 exists to catch, so the two stay apart:
    /// two facts, two fields. Reset to `None` on every spawn.
    pub launch_signal_at: Option<i64>,
    /// Did #215's config injection actually happen for this spawn (a
    /// non-empty `Injection`, per `harness_ensure_running`'s resolved `injected`
    /// field)? Reset to `false` on every spawn and set once
    /// `harness_ensure_running` resolves: the window between the two is "we don't
    /// know yet," which the #238 nudge gate treats the same as "no."
    /// Without #215 there is no proof the harness's CLI even has the
    /// review MCP tools wired up, let alone that a pasted nudge will
    /// reach an agent that can act on it.
    pub injected: bool,
    /// #277 (epic #298): epoch ms the main transcript's end of turn
    /// was deferred because `subagents.workingCount` was non-zero at
    /// the time (subagents plus background tasks, #446), or `None` when
    /// no deferral is armed. Arming changes
    /// no phase: the harness stays whatever it already was (`running`
    /// or `permission`). Disarmed by any main-transcript work signal
    /// (`setRunningFromAdapter`/`setWaitingFromAdapter`/`exited`, plus
    /// a fresh `spawned` record starts with it unset): see
    /// `awaitingPromptFromAdapter` and the tick's Rules 3/4 for how a
    /// deferral eventually resolves on its own. In-memory only, reset
    /// on every spawn.
    pub delegation_deferred_at: Option<i64>,
    /// #277: epoch ms of the most recent subagent event of any kind
    /// (start/tool-result/end): what the Rule 4 ceiling measures
    /// silence against. Bumped by `noteSubagentStarted` (live starts
    /// only) and `noteSubagentActivity`; both mutate silently, the way
    /// `record_output` bumps `last_output_at`. In-memory only, reset on
    /// every spawn.
    pub delegation_activity_at: i64,
    /// #277: epoch ms the tick first observed the working-subagent set
    /// empty while a deferral was armed, or `None`. What the Rule 3
    /// settle timer measures against. **The tick is the only writer**:
    /// it sets this when it observes empty and clears it if the set
    /// becomes non-empty again, kept that way deliberately: one
    /// writer is what keeps this field debuggable. In-memory only,
    /// reset on every spawn.
    pub delegation_emptied_at: Option<i64>,
    /// #277: how many subagents have started live since the user's
    /// last prompt to this harness. Consumed by the notification
    /// wording ("N delegated agents finished"); reset by `spawned` and
    /// by `noteUserPrompt`. In-memory only.
    ///
    /// Deliberately cumulative across a flush: nothing resets it when
    /// the phase moves to `waiting` (settle, ceiling, or a plain
    /// adapter end-of-turn with no deferral), only the next prompt
    /// does. That's what makes "3 delegated agents finished" true for
    /// the whole prompt cycle rather than just since the last flush.
    /// A future call site that flips the phase without going through
    /// `noteUserPrompt` would silently reintroduce a stale count.
    pub delegated_count: u32,
    /// #404: has a lost-first-paste silence already been recovered once
    /// for this spawn (`recoverUnheardSilence`)? A mail
    /// nudge (or any #238 seam submit) into a harness whose only proof of
    /// life was the launch signal can arm the #259 watchdog and then have
    /// its own submit rolled back by #388's settlement before the tail
    /// ever gets a real prompt to answer: that degrade is a false
    /// alarm, not evidence the adapter is broken, and this field is what
    /// lets it be undone exactly once. A SECOND silent seam submit on the
    /// same spawn, after this is already `true`, is real evidence the
    /// tail is watching the wrong file, so `recoverUnheardSilence` refuses
    /// once this is set: a stuck harness must still degrade for good.
    /// In-memory only, reset to `false` on every spawn.
    pub silence_recovered: bool,
    /// #423: epoch ms of the last REAL phase change (`set_phase` with a
    /// different phase; a patch-only re-entry doesn't count). Spawn time
    /// until the first change. What the supervisor reads to tell how long
    /// a harness has sat in its current phase. In-memory only.
    pub phase_since: i64,
    /// #423: the main session's latest adapter turn signal: "work" from
    /// `setRunningFromAdapter`, "end" from `awaitingPromptFromAdapter` /
    /// `setWaitingFromAdapter`. Recorded REGARDLESS of whether the call
    /// then changed phase or was ignored (non-authoritative, `permission`
    /// without `clearsPermission`, ...): the point is what the adapter
    /// said, not what Skein did with it. Subagent events never record it.
    /// `None` until the first signal. In-memory only, reset on every spawn.
    pub last_turn_signal: Option<TurnSignal>,
    /// #423: the most recent adapter event of any kind, stamped by
    /// `adapter_delivered` on every call. `restores_authority` is whether
    /// that event was one allowed to hand authority back (false for the
    /// `session_end` that announces the adapter's loss). `None` until the
    /// adapter first speaks. In-memory only, reset on every spawn.
    pub last_adapter_event: Option<AdapterEvent>,
    /// #423: epoch ms `authoritative` last went true -> false (detach, or
    /// either silent-adapter watchdog). Left in place when authority is
    /// later regained, so a reader compares it with `last_adapter_event`.
    /// `None` if authority was never lost. In-memory only, reset on every
    /// spawn.
    pub authority_lost_at: Option<i64>,
    /// #423: epoch ms the `permission` phase was entered or the permission
    /// ping last arrived (`set_permission_from_adapter`, including a re-entry
    /// while already in `permission`). Cleared to `None` whenever the phase
    /// leaves `permission`, in the same place `permission_tool` is.
    /// In-memory only.
    pub permission_at: Option<i64>,
    /// #423: epoch ms of the user's last Enter/submit in this PTY (typed, or a
    /// `sendPrompt` seam submit), recorded whatever the watchdog does with it.
    /// `None` until the first. In-memory only, reset on every spawn.
    pub last_submit_at: Option<i64>,
}

impl HarnessActivity {
    /// A fresh record in `phase`, every other field at its spawn default.
    /// `spawned` uses `Spawning`; `exited` for a harness never seen to spawn
    /// uses `Exited` with the code.
    #[must_use]
    pub fn fresh(phase: ActivityPhase, exit_code: Option<i32>, now_ms: i64) -> Self {
        Self {
            phase,
            last_output_at: None,
            exit_code,
            spawned_at: now_ms,
            has_user_input: false,
            authoritative: false,
            tail: String::new(),
            permission_tool: None,
            permission_agent_type: None,
            permission_agent_id: None,
            question_open: false,
            adapter_heard: false,
            prompt_submitted_at: None,
            adapter_silent: false,
            degraded_by: None,
            launch_signal_at: None,
            injected: false,
            delegation_deferred_at: None,
            delegation_activity_at: now_ms,
            delegation_emptied_at: None,
            delegated_count: 0,
            silence_recovered: false,
            phase_since: now_ms,
            last_turn_signal: None,
            last_adapter_event: None,
            authority_lost_at: None,
            permission_at: None,
            last_submit_at: None,
        }
    }
}

/// One real phase change, queued by `set_phase` for the driver to drain
/// (the TS `TransitionListener`, as an outbox).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub harness_id: String,
    pub from: ActivityPhase,
    pub to: ActivityPhase,
    pub source: TransitionSource,
    pub at_ms: i64,
    /// `has_user_input` of the record right after the change, which is
    /// when the TS listeners read it.
    pub has_user_input: bool,
}

/// Declares [`TransitionSource`] with its `as_str()` and serde names from
/// one list, so the three can't drift apart.
macro_rules! transition_sources {
    ($( $(#[$doc:meta])* $variant:ident => $s:literal, )*) => {
        /// Which detection strategy or lifecycle event fired a given phase
        /// transition. Persisted to `harness_events.source` and surfaced as
        /// the "why" chip in the L7 activity feed. Mirrors the TS
        /// `TRANSITION_SOURCE` table value for value.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum TransitionSource {
            $( $(#[$doc])* #[serde(rename = $s)] $variant, )*
        }

        impl TransitionSource {
            /// The exact TS string.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $s, )* }
            }
        }
    };
}

transition_sources! {
    /// PTY-driven (L1 substrate).
    PtyOutput => "pty-output",
    PtyExit => "pty-exit",
    /// #731: a view re-attached to a process that was already running; not news.
    /// L2a: idle heuristic on the worktree-watcher tick.
    L2aIdle => "l2a-idle",
    /// L2b: generic prompt-pattern fallback.
    L2bPattern => "l2b-pattern",
    L2bDrained => "l2b-drained",
    /// #623: the rendered screen says so (Hermes never goes byte-quiet).
    L2bScreen => "l2b-screen",
    L2bScreenCleared => "l2b-screen-cleared",
    /// L2c-1: Claude JSONL adapter.
    L2c1ClaudeAssistant => "l2c1-claude-assistant",
    L2c1ClaudeEndTurn => "l2c1-claude-end-turn",
    L2c1ClaudeToolUse => "l2c1-claude-tool-use",
    L2c1ClaudeToolResult => "l2c1-claude-tool-result",
    L2c1ClaudeUserPrompt => "l2c1-claude-user-prompt",
    /// L2c-2: opencode SSE adapter.
    L2c2OpencodeBusy => "l2c2-opencode-busy",
    L2c2OpencodeIdle => "l2c2-opencode-idle",
    /// #175: assumed-idle baseline after an SSE (re)connect: a guess,
    /// never notify-worthy (see `isNotifiable`).
    L2c2OpencodeBaseline => "l2c2-opencode-baseline",
    L2c2OpencodeMessageDelta => "l2c2-opencode-message-delta",
    L2c2OpencodeToolUse => "l2c2-opencode-tool-use",
    /// #86: permission promoted to its own phase.
    L2c1ClaudePermission => "l2c1-claude-permission-request",
    L2c2OpencodePermission => "l2c2-opencode-permission-asked",
    L2c2OpencodePermissionReplied => "l2c2-opencode-permission-replied",
    L2c2OpencodeQuestion => "l2c2-opencode-question-asked",
    L2c2OpencodeQuestionResolved => "l2c2-opencode-question-resolved",
    /// The only "answered" signal Claude gives us for its own dialog:
    /// the user typed something decisive into the PTY. See
    /// `is_decisive_input`.
    UserInputPermission => "user-input-permission",
    /// #86: the adapter that could have resolved a permission dialog
    /// went away. See `release_permission`.
    AdapterDetached => "adapter-detached",
    /// #259: the adapter never delivered anything after a prompt was
    /// submitted: the watchdog handed the harness back to L2a.
    AdapterSilent => "adapter-silent",
    /// #273: no launch signal ever arrived (and the tail hadn't spoken
    /// either) within `LAUNCH_SILENT_AFTER_MS` of spawn: the watchdog
    /// handed the harness back to L2a. Deliberately its own source
    /// rather than reusing `AdapterSilent`: the two are diagnosed
    /// differently (no prompt was even needed to arm this one), and this
    /// table exists so the L7 feed's "why" chip says which.
    LaunchSilent => "launch-silent",
    /// #273: the harness's own CLI reported its own launch: Claude's
    /// `SessionStart` hook via #215 injection. See `note_launch_signal`.
    L2c1ClaudeSessionStart => "l2c1-claude-session-start",
    /// #116: `/clear` starts a brand new session id mid-process: see
    /// `session_switched`.
    L2c1ClaudeSessionClear => "l2c1-claude-session-clear",
    /// #116: an in-tool `/resume` (or a pre-v2.1.214 fork, which reported
    /// itself the same way) re-points onto a different session id: see
    /// `session_switched`.
    L2c1ClaudeSessionResume => "l2c1-claude-session-resume",
    /// #116: a `/branch`, `/fork`, `--fork-session` or desktop rewind
    /// reports its own dedicated `source: "fork"` from Claude Code
    /// v2.1.214 on, carrying the newly forked session id: see
    /// `session_switched`.
    L2c1ClaudeSessionFork => "l2c1-claude-session-fork",
    /// #298: a subagent's own tool result proves the gate its permission
    /// prompt held is gone, even though the main transcript never sees
    /// it. See `clear_permission`.
    L2c1ClaudeSubagentToolResult => "l2c1-claude-subagent-tool-result",
    /// #277: a deferred end-of-turn resolving on its own, via the tick's
    /// Rule 3 (working set went empty and stayed empty) or Rule 4
    /// (ceiling: a signal was presumed lost, or a task held too long). Deliberately
    /// NOT prefixed `l2c1-claude-`: the deferral mechanism is
    /// harness-agnostic, keyed on outstanding work (`subagents` plus
    /// `backgroundTasks` working counts, #446), so if opencode ever grows
    /// a liveness signal feeding the same registries, it gets this
    /// behaviour (and these source names) for free.
    DelegationSettled => "delegation-settled",
    DelegationCeiling => "delegation-ceiling",
    /// #441: the stale-work watchdog. A `running` authoritative harness
    /// with outstanding work but no deferral armed and no signal for the
    /// ceiling is presumed to have lost its end; nothing is claimed
    /// finished.
    WorkWatchdog => "work-watchdog",
    /// #634: a live subagent/background start landed after the main end of
    /// turn had read `waiting`; `reclaimDelegationFromAdapter` re-arms the
    /// deferral and returns to `running`. waiting -> running, never notifiable.
    DelegationResumed => "delegation-resumed",
    /// #404: `recoverUnheardSilence` undoing a #259 adapter-silent degrade
    /// that turns out to have been a lost first paste, not a broken tail:
    /// see that method's own doc for the conditions.
    SilenceRecovered => "silence-recovered",
    /// #423: the harness supervisor moved a harness whose phase had gone
    /// stale (`supervisorSetWaiting`).
    SupervisorRecovered => "supervisor-recovered",
    /// #625: L2c-3, the Hermes shell-hook adapter. First prompt of the
    /// process (`on_session_start`); also the launch signal.
    L2c3HermesSessionStart => "l2c3-hermes-session-start",
    /// The screen reader's `ready` status line stood in for the launch signal
    /// Hermes does not send (its first hook fires at the first prompt).
    L2c3HermesReady => "l2c3-hermes-ready",
    /// `pre_llm_call` / `pre_tool_call` / `on_session_start`.
    L2c3HermesRunning => "l2c3-hermes-running",
    /// `on_session_end`: fires at the end of every turn.
    L2c3HermesEndTurn => "l2c3-hermes-end-turn",
    /// `on_session_end` with `interrupted` (Ctrl+C); the TUI stays open.
    L2c3HermesInterrupted => "l2c3-hermes-interrupted",
    /// A real (non-`smart`) `pre_approval_request`.
    L2c3HermesPermission => "l2c3-hermes-approval-request",
    /// The matching `post_approval_response`.
    L2c3HermesPermissionAnswered => "l2c3-hermes-approval-response",
    /// A dialog answered after the main turn had already ended (a child's
    /// approval): back to `waiting`, from `permission`, so never notifiable.
    L2c3HermesPermissionAnsweredIdle => "l2c3-hermes-approval-response-idle",
    /// #490: the user (or an agent on their behalf) restarted the harness;
    /// logged straight to harness_events by `restartHarness`, not through
    /// the phase store.
    UserRestart => "user-restart",
    /// #491: the same restart, started by the Claude Code update notice's
    /// auto mode rather than by a person.
    VersionAutoRestart => "version-auto-restart",
}
