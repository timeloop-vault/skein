// Per-harness activity state machine + event hook (epic #50, L1+L3+L2a).
//
// The single source of truth for "what is this harness doing right
// now?" — read by the bottom status bar (#29), by the harness tab
// dots, and (in follow-on PRs) by the notification surfaces (#12),
// the per-room aggregate (#50 L4), and the cross-harness activity
// feed (#50 L7).
//
// Today we have exactly one signal flowing from a harness: bytes
// over the PTY `Channel<String>`. This module derives a real state
// machine from that signal using the idle-heuristic strategy (#50
// L2a): output → `running`, sustained silence → `idle`, PTY exit
// → `exited`. Pattern-match and harness-native strategies (L2b /
// L2c) will plug in later by calling the same `setPhase` mutator —
// consumers won't know or care which strategy fed the transition.
//
// Why not Rust-side: this state is purely derived from a stream
// the frontend already receives. Putting the model in Rust would
// add a second IPC channel and split logic across the boundary
// for no win. If we ever need cross-restart persistence (epic L6)
// the natural shape is "frontend emits transitions, Rust appends
// to a log" — the state machine itself can stay here.

import { useCallback, useSyncExternalStore } from "react";
import { matchesWaitingPrompt, stripAnsi } from "./harnessPatterns.ts";
import { subagents } from "./subagents.ts";
import type { Status } from "./types.ts";

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

/// Sustained silence threshold for `running → idle`. Hard-coded for
/// v1; epic #50 L5e moves this into Settings.
const IDLE_AFTER_MS = 8_000;
/// Shorter silence threshold for `running → waiting` via L2b
/// pattern match. The user types `sudo X`, "Password:" appears,
/// output stops — we want the dot to flip blue near-instantly, not
/// 8 s later. 500 ms is generous enough to avoid firing during
/// mid-stream output that happens to contain a prompt-looking
/// substring, while tight enough to feel responsive.
const PATTERN_WAITING_AFTER_MS = 500;
/// Maximum tail-buffer size per harness. The matcher only scans
/// the last 256 chars; we keep more to handle large chunks that
/// arrive in one PTY read (xterm splits at no fixed boundary) but
/// cap so a chatty shell session doesn't grow memory unbounded.
const TAIL_MAX_CHARS = 2_048;
/// How often the background tick scans for idle transitions. A
/// faster tick gives tighter detection latency; 1s strikes a
/// sensible balance — at worst the user sees "idle" up to a second
/// late, which is below the perceptual threshold for a status dot.
const TICK_INTERVAL_MS = 1_000;
/// Window during which PTY output is treated as "our fault, not the
/// child's." Triggered explicitly by callers (e.g. LiveTerminal on
/// visibility flip — `term.focus()` sends a focus-in event to the
/// child, which many TUIs answer with a full redraw). Without this,
/// switching to a long-idle harness pops it back to `running` for 8s
/// before settling, which contradicts the actual state. 800 ms is
/// generous enough to cover slow repaints; if real output arrives
/// after the window, the normal path kicks back in.
const INDUCED_MUTE_MS = 800;
/// How long an attached adapter may stay silent after the user submits
/// a prompt before the watchdog gives up on it (#259). Claude writes
/// the prompt row the moment it is submitted and opencode's stream
/// says `connected` before anything else, so a healthy adapter speaks
/// well inside this. Gated on a prompt rather than on spawn because
/// Claude creates no transcript at all until the first one: a fresh
/// harness left at its prompt is healthy, not silent.
const ADAPTER_SILENT_AFTER_MS = 10_000;
/// #277 (epic #298), Rule 3 — how long a deferred end-of-turn waits,
/// once the working-subagent set is observed empty, before flushing to
/// `waiting` on its own. Measured on 1009 real delegations: after a
/// subagent's transcript reaches a terminal row, the main session's
/// next row (proof it woke up and is working the delegation's result)
/// appears in 27 ms median, 1.7 s p90, 54.1 s max — every one of the
/// 1009 woke up. 60 s sits comfortably above the worst observed
/// wake-up, so this only ever fires when the session genuinely did not
/// come back.
const DELEGATION_SETTLE_MS = 60_000;
/// #277, Rule 4 — the safety net for a lost subagent signal: while a
/// deferral is armed and the working set is non-empty, no event from
/// any of those subagents for this long presumes them gone. Measured
/// on 1038 real subagents: 12.4% have an internal quiet gap over 2
/// min, 4.4% over 5 min, 2.4% over 10 min (p99 ≈ 2 h) — ordinary tool
/// calls, not stuck sessions. A false "presumed gone" only costs one
/// early notification (today's behaviour), so the cost is asymmetric
/// and the constant leans long rather than risk a permanently
/// suppressed harness.
const DELEGATION_CEILING_MS = 15 * 60_000;

const store = new Map<string, HarnessActivity>();
const listeners = new Map<string, Set<() => void>>();
const muteUntil = new Map<string, number>();
let tickHandle: ReturnType<typeof setInterval> | null = null;

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
const transitionListeners = new Set<TransitionListener>();

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

const emit = (id: string): void => {
	const set = listeners.get(id);
	if (!set) return;
	for (const cb of set) cb();
};

/// Set equality for the permission-ids snapshot below — only used to
/// decide whether a fresh Set needs allocating, since
/// `useSyncExternalStore` requires a referentially stable snapshot
/// when nothing actually changed.
const setsEqual = (a: ReadonlySet<string>, b: ReadonlySet<string>): boolean => {
	if (a.size !== b.size) return false;
	for (const id of a) if (!b.has(id)) return false;
	return true;
};

/// Every harness id currently in `permission`, across every room.
/// Rebuilt (not incrementally maintained) on every transition that
/// touches the phase — the store is small enough that a full scan is
/// cheap, and it keeps this correct by construction instead of by
/// careful bookkeeping at each of the several call sites that can
/// enter or leave `permission`. #86.
let permissionIds: ReadonlySet<string> = new Set();
const permissionListeners = new Set<() => void>();

const recomputePermissionIds = (): void => {
	const next = new Set<string>();
	for (const [id, a] of store) {
		if (a.phase === "permission") next.add(id);
	}
	if (setsEqual(next, permissionIds)) return;
	permissionIds = next;
	for (const cb of permissionListeners) cb();
};

const setPhase = (
	id: string,
	phase: ActivityPhase,
	source: TransitionSource,
	patch?: Partial<HarnessActivity>,
): void => {
	const cur = store.get(id);
	if (!cur) return;
	// Suppress no-op transitions so consumers don't churn on
	// continuous output (every chunk would otherwise emit). Only
	// real phase changes notify subscribers; `lastOutputAt`
	// mutates silently inside `recordOutput`. A patch-only call (e.g.
	// re-entering `permission` with a fresh tool name) still goes
	// through even when the phase itself didn't change, so `.get()`
	// reflects the new tool.
	if (cur.phase === phase && !patch) return;
	const from = cur.phase;
	// Leaving `permission` drops the stale tool name by default; a
	// caller reporting a fresh one (re-entering `permission`) passes
	// it in `patch`, which is spread after and wins.
	const leftPermission = from === "permission" && phase !== "permission";
	store.set(id, {
		...cur,
		phase,
		...(leftPermission
			? { permissionTool: null, permissionAgentType: null, permissionAgentId: null }
			: null),
		...patch,
	});
	emit(id);
	if (from !== phase) {
		for (const cb of transitionListeners) cb(id, from, phase, source);
	}
	if (from === "permission" || phase === "permission") {
		recomputePermissionIds();
	}
};

/// The adapter never spoke after a prompt was submitted: give up on it.
/// Without this an adapter watching the wrong place freezes the harness
/// for good — authority silences PTY output, the tick skips it, and it
/// never leaves `spawning` (#259, a JSONL path encoded differently from
/// Claude's). `spawning` is moved on explicitly because the tick below
/// only ever acts on `running`; `lastOutputAt` restarts so the idle
/// window is measured from here, not from spawn.
const degradeSilentAdapter = (id: string, cur: HarnessActivity, now: number): void => {
	console.warn(
		`[skein] harness ${id}: adapter delivered nothing ${ADAPTER_SILENT_AFTER_MS / 1000}s after a prompt; falling back to the idle heuristic`,
	);
	store.set(id, { ...cur, authoritative: false, adapterSilent: true, lastOutputAt: now });
	if (cur.phase === "spawning") {
		setPhase(id, "running", TRANSITION_SOURCE.AdapterSilent);
	}
};

/// #277: void an armed end-of-turn deferral. Called from every
/// main-transcript work/settle signal — `setRunningFromAdapter`,
/// `setWaitingFromAdapter`, `exited` — so the deferral can never
/// survive past whatever event proves the session isn't waiting on
/// its subagents anymore. Mutates in place, the way `recordOutput`
/// bumps `lastOutputAt`: this is bookkeeping, not a phase change, so
/// it must not emit — a caller that goes on to call `setPhase` gets
/// its own emit from that.
///
/// Deliberately unconditional on the harness's current phase: Rule 2
/// requires this to run even when the caller it's called from is
/// about to no-op (`setRunningFromAdapter` while still `permission`
/// without `clearsPermission`, or `setPhase`'s own same-phase
/// short-circuit) — the disarm must not depend on a phase write
/// actually happening.
const disarmDelegation = (id: string): void => {
	const cur = store.get(id);
	if (!cur || cur.delegationDeferredAt === null) return;
	cur.delegationDeferredAt = null;
	cur.delegationEmptiedAt = null;
};

const ensureTick = (): void => {
	if (tickHandle !== null) return;
	tickHandle = setInterval(() => {
		const now = Date.now();
		for (const [id, a] of store) {
			// #277: a deferred end-of-turn is evaluated before the
			// `authoritative` skip below, because a deferred harness
			// IS authoritative by definition — only the Claude
			// translator's `awaitingPromptFromAdapter` arms one, and
			// only while an L2c adapter is attached. `continue`s out
			// either way, so the authoritative block never
			// double-handles it.
			if (a.delegationDeferredAt !== null) {
				// #86: permission is the harder stop. Leave the
				// harness alone; re-evaluate on a later tick once the
				// dialog clears back to `running`.
				if (a.phase === "permission") continue;
				const working = subagents.workingCount(id);
				if (working === 0) {
					// Rule 3 — settle timer. See `DELEGATION_SETTLE_MS`
					// for the measurement behind the threshold.
					if (a.delegationEmptiedAt === null) {
						a.delegationEmptiedAt = now;
					} else if (now - a.delegationEmptiedAt >= DELEGATION_SETTLE_MS) {
						a.delegationDeferredAt = null;
						a.delegationEmptiedAt = null;
						setPhase(id, "waiting", TRANSITION_SOURCE.DelegationSettled);
					}
				} else {
					// The working set is non-empty again after having
					// been seen empty — clear the mark so a fresh
					// settle window starts cleanly if it empties again.
					if (a.delegationEmptiedAt !== null) a.delegationEmptiedAt = null;
					// Rule 4 — ceiling. See `DELEGATION_CEILING_MS` for
					// the measurement behind the threshold.
					if (now - a.delegationActivityAt >= DELEGATION_CEILING_MS) {
						const dropped = subagents.presumeGone(id);
						console.warn(
							`[skein] harness ${id}: presumed ${dropped} subagent(s) gone after ` +
								`${DELEGATION_CEILING_MS / 60_000} min with no subagent event; flushing the deferred end of turn`,
						);
						a.delegationDeferredAt = null;
						a.delegationEmptiedAt = null;
						setPhase(id, "waiting", TRANSITION_SOURCE.DelegationCeiling);
					}
				}
				continue;
			}
			// Harnesses with an authoritative source (Claude JSONL
			// tail, opencode SSE adapter) write their own phase
			// from the L2c module. The idle heuristic would fight
			// the adapter — e.g. when Claude is mid-turn but the
			// PTY is briefly quiet during a tool call, L2a would
			// flip to `idle` while L2c says `running`. Skip these,
			// unless the adapter has proven silent (#259).
			if (a.authoritative) {
				if (
					!a.adapterHeard &&
					a.promptSubmittedAt !== null &&
					now - a.promptSubmittedAt >= ADAPTER_SILENT_AFTER_MS
				) {
					degradeSilentAdapter(id, a, now);
				}
				continue;
			}
			if (a.phase !== "running" || a.lastOutputAt === null) continue;
			const quiet = now - a.lastOutputAt;
			// L2b: pattern-match → waiting on a shorter threshold
			// than idle. Triggered when output has been quiet for
			// PATTERN_WAITING_AFTER_MS (500 ms) AND the tail buffer
			// matches a known prompt regex (sudo password, [y/n],
			// "Press Enter", …). The shorter threshold makes the
			// dot flip blue near-instantly when a sudo prompt
			// appears, instead of waiting the full 8 s idle window.
			//
			// If the tail doesn't match a known prompt, the
			// harness falls through to the L2a `→ idle` check
			// below — same behaviour as pre-L2b.
			if (quiet >= PATTERN_WAITING_AFTER_MS && matchesWaitingPrompt(a.tail)) {
				setPhase(id, "waiting", TRANSITION_SOURCE.L2bPattern);
				continue;
			}
			if (quiet >= IDLE_AFTER_MS) {
				setPhase(id, "idle", TRANSITION_SOURCE.L2aIdle);
			}
		}
	}, TICK_INTERVAL_MS);
};

const stopTickIfIdle = (): void => {
	if (store.size === 0 && tickHandle !== null) {
		clearInterval(tickHandle);
		tickHandle = null;
	}
};

/// Is this keystroke "decisive" enough to count as answering a
/// permission dialog (#86)? Claude gives us no explicit "answered"
/// signal — approval and denial both just let the tool proceed or
/// error out, and that can take minutes — so the only prompt signal
/// left is the user's own keystroke. Deliberately narrow: a lone ESC
/// (dismiss) counts, but an escape *sequence* (arrow keys `\x1b[A`,
/// xterm's focus events `\x1b[I` / `\x1b[O`) must not, since those
/// fire on ordinary navigation and window-focus changes that have
/// nothing to do with answering anything. Exported and pure so it can
/// be unit tested without the store.
export function isDecisiveInput(data: string): boolean {
	if (data.includes("\r") || data.includes("\n")) return true;
	if (data === "\x1b") return true;
	if (data.includes("\x03")) return true;
	if (data.length === 1 && /^[0-9yYnN]$/.test(data)) return true;
	return false;
}

export const harnessActivity = {
	/// Record a fresh spawn. Call from LiveTerminal once `pty_spawn`
	/// resolves successfully.
	spawned(id: string): void {
		const now = Date.now();
		store.set(id, {
			phase: "spawning",
			lastOutputAt: null,
			exitCode: null,
			spawnedAt: now,
			hasUserInput: false,
			authoritative: false,
			tail: "",
			permissionTool: null,
			permissionAgentType: null,
			permissionAgentId: null,
			adapterHeard: false,
			promptSubmittedAt: null,
			adapterSilent: false,
			injected: false,
			delegationDeferredAt: null,
			delegationActivityAt: now,
			delegationEmptiedAt: null,
			delegatedCount: 0,
		});
		ensureTick();
		emit(id);
	},

	/// Record whether #215's config injection happened for this spawn.
	/// Call once `pty_spawn` resolves — LiveTerminal is the only
	/// caller, and only for the spawn that just succeeded. No-op for a
	/// harness we've already forgotten (a cancelled spawn racing
	/// unmount).
	setInjected(id: string, injected: boolean): void {
		const cur = store.get(id);
		if (!cur || cur.injected === injected) return;
		store.set(id, { ...cur, injected });
		emit(id);
	},

	/// Mark a harness as having an authoritative L2c adapter
	/// attached (Claude JSONL tail, opencode event stream, …).
	/// While set, the L2a idle tick skips this harness — only
	/// `setRunningFromAdapter` / `setWaitingFromAdapter` and
	/// `exited` write its phase. Idempotent.
	attachAuthoritativeSource(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.authoritative) return;
		store.set(id, { ...cur, authoritative: true });
	},

	/// The L2c adapter delivered an event — call before translating
	/// every one. Disarms the silent-adapter watchdog for this spawn,
	/// and if the watchdog had already given up (a false alarm: say the
	/// Enter answered a trust dialog and no prompt followed in time),
	/// hands authority back so the adapter's phases win again. #259.
	adapterDelivered(id: string): void {
		const cur = store.get(id);
		if (!cur || (cur.adapterHeard && !cur.adapterSilent)) return;
		if (cur.adapterSilent) {
			console.info(`[skein] harness ${id}: adapter recovered; handing phase back to it`);
		}
		store.set(id, {
			...cur,
			adapterHeard: true,
			adapterSilent: false,
			...(cur.adapterSilent ? { authoritative: true } : null),
		});
	},

	/// Adapter detached — fall back to the L2a heuristic for this
	/// harness. The next chunk or tick will re-evaluate.
	///
	/// #277: also disarms any armed deferral. An adapter that is gone
	/// can never resolve the end of turn it deferred — its own
	/// `awaiting_prompt` won't come again, and neither will the
	/// subagent events `delegationActivityAt` needs, since (today) the
	/// same tail feeds both. Left armed, the deferral would keep
	/// steering the harness's phase — Rule 3/4 evaluate it BEFORE the
	/// tick's `authoritative` skip — even though authority has already
	/// reverted to L2a, so the dot would read `running` /
	/// "delegating · N agents" regardless of PTY truth for up to
	/// `DELEGATION_CEILING_MS`, instead of `session_end`'s promised
	/// fallback to chunk-driven detection. Called unconditionally,
	/// ahead of the `authoritative` guard below: `disarmDelegation` is
	/// itself idempotent (no-ops once `delegationDeferredAt` is
	/// already `null`), and a second detach call on an already-
	/// detached harness has nothing left to disarm, so running it
	/// on that path too is free, not merely harmless.
	detachAuthoritativeSource(id: string): void {
		disarmDelegation(id);
		const cur = store.get(id);
		if (!cur || !cur.authoritative) return;
		store.set(id, { ...cur, authoritative: false });
	},

	/// L2c adapter says the harness is doing work. Bypasses the
	/// recordOutput throttle so the transition fires on the
	/// adapter event boundary, not on the next PTY chunk. The
	/// `source` identifies which adapter event drove the transition
	/// (e.g. `l2c1-claude-assistant`, `l2c2-opencode-busy`) and
	/// flows through to the persisted event log for L7 attribution.
	///
	/// #86: a harness sitting in `permission` is hard-blocked on a
	/// dialog the user hasn't answered yet. Most adapter events that
	/// call this just mean "still working" (a fresh assistant turn, the
	/// next tool queued) and must not paper over that — so by default
	/// this is a no-op while in `permission`. Only a signal that
	/// genuinely means the gate is gone (a tool result landed, the
	/// user submitted a fresh prompt) should pass `clearsPermission:
	/// true`; the translator at each call site decides which it is.
	setRunningFromAdapter(
		id: string,
		source: TransitionSource,
		opts?: { clearsPermission?: boolean },
	): void {
		// #277 Rule 2: any main-transcript work signal disarms a
		// deferred end-of-turn, even one this call is about to no-op
		// on below (still `permission` without `clearsPermission`) —
		// the disarm must not depend on `setPhase` actually running.
		disarmDelegation(id);
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		if (cur.phase === "permission" && !opts?.clearsPermission) return;
		setPhase(id, "running", source);
	},

	/// L2c adapter says the harness is awaiting user input. Bypasses
	/// the chunk throttle for the same reason. Always allowed to leave
	/// `permission` — unlike `setRunningFromAdapter`, every signal that
	/// calls this (Claude's end-of-turn, opencode's session-idle) means
	/// the harness is unambiguously done with whatever it was doing,
	/// permission gate included. #86.
	setWaitingFromAdapter(id: string, source: TransitionSource): void {
		// #277 Rule 2: unconditional disarm, same reasoning as
		// `setRunningFromAdapter` above.
		disarmDelegation(id);
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "waiting", source);
	},

	/// #277 (epic #298): the Claude translator's `awaiting_prompt` arm
	/// calls this instead of `setWaitingFromAdapter` directly. The main
	/// transcript ended its turn, but that is a lie about the harness
	/// being done if it just delegated work still running in the
	/// background — measured true in 94% of 127 real sessions.
	///
	/// No working subagents (`subagents.workingCount` is 0): identical
	/// to today, straight to `waiting`. One or more: arm the deferral
	/// (idempotent — a second end-of-turn while already deferred
	/// doesn't reset when it was armed) and change NO phase at all.
	/// The harness is already `running`, or `permission` — which
	/// outranks this regardless, so simply not calling `setPhase`
	/// leaves it alone either way. See the tick's Rules 3/4 for how a
	/// deferral eventually resolves without a further adapter event.
	awaitingPromptFromAdapter(id: string, source: TransitionSource): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		if (subagents.workingCount(id) === 0) {
			harnessActivity.setWaitingFromAdapter(id, source);
			return;
		}
		if (cur.delegationDeferredAt === null) {
			cur.delegationDeferredAt = Date.now();
		}
	},

	/// L2c adapter reports a permission dialog is on screen — Claude's
	/// PermissionRequest hook fired, or opencode's permission-asked SSE
	/// event landed. `toolName` is `null` when the adapter can't say
	/// (opencode's event carries no tool name); every notification
	/// surface shows it when present. `agentType` names the subagent
	/// the dialog belongs to when it isn't the main session (#298);
	/// omitted or `null` for the main session or when the adapter
	/// can't say. `agentId` is that same subagent's id — what
	/// `clearPermission` correlates on — omitted or `null` likewise.
	/// No-op once exited. Always goes through `setPhase` even when
	/// already in `permission`, so a second request with a different
	/// tool name still updates `.permissionTool` for anything reading
	/// `.get()` live. #86.
	setPermissionFromAdapter(
		id: string,
		source: TransitionSource,
		toolName: string | null,
		agentType?: string | null,
		agentId?: string | null,
	): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		setPhase(id, "permission", source, {
			permissionTool: toolName,
			permissionAgentType: agentType ?? null,
			permissionAgentId: agentId ?? null,
		});
	},

	/// Record user input (keystroke / paste) on a harness. `data` is
	/// the exact bytes that keystroke sends to the child (xterm's
	/// `onKey` reports the same string `onData` would carry for it —
	/// real user input, not an auto-response the terminal generates for
	/// a device query).
	///
	/// Two independent effects: flips `hasUserInput` to true exactly
	/// once per spawn (subsequent keystrokes are no-ops there); and, if
	/// the harness is blocked on `permission`, checks whether `data` is
	/// "decisive" (see `isDecisiveInput`) and if so moves it to
	/// `running`. Claude gives no "the dialog was answered" signal of
	/// its own — the only way the user can answer it is by typing into
	/// the PTY, so that keystroke IS the signal (#86). Doesn't emit for
	/// the `hasUserInput` flip alone — consumers that care read the
	/// flag lazily at transition time.
	///
	/// #259: the first Enter on a harness whose attached adapter has
	/// said nothing yet arms the silent-adapter watchdog.
	recordInput(id: string, data: string): void {
		const cur = store.get(id);
		if (!cur) return;
		const armWatchdog =
			cur.authoritative &&
			!cur.adapterHeard &&
			cur.promptSubmittedAt === null &&
			(data.includes("\r") || data.includes("\n"));
		if (!cur.hasUserInput || armWatchdog) {
			store.set(id, {
				...cur,
				hasUserInput: true,
				...(armWatchdog ? { promptSubmittedAt: Date.now() } : null),
			});
		}
		if (cur.phase === "permission" && isDecisiveInput(data)) {
			setPhase(id, "running", TRANSITION_SOURCE.UserInputPermission);
		}
	},

	/// Leave `permission` for `running`, and touch no other phase. For
	/// an adapter detaching mid-dialog: nothing it would have sent can
	/// arrive now, and the L2a tick never moves a non-running phase, so
	/// without this the harness would read "permission needed" until
	/// the user typed or the PTY died. `running` hands it back to L2a.
	releasePermission(id: string, source: TransitionSource): void {
		if (store.get(id)?.phase !== "permission") return;
		setPhase(id, "running", source);
	},

	/// A subagent's tool result landed (#298). Proves *that subagent's*
	/// gate is gone, and only that one — with several subagents running
	/// concurrently, an unrelated subagent finishing its own tool call
	/// must not clear a different subagent's still-open dialog. No-op
	/// unless the harness is actually in `permission`. When it is:
	/// `agentId` is compared against the stored `permissionAgentId` —
	/// a match clears; a mismatch (a *different* subagent's result) is
	/// left alone. A stored `null` still clears unconditionally, for
	/// two folded-together cases that read the same way from here: a
	/// main-session dialog (there is no subagent to disambiguate
	/// against), and an adapter event where `agentId` wasn't reported
	/// at all — the field is confirmed live as of 2026-09-20, so that
	/// second case is now the rare one (injection off, or a future CLI
	/// change), but falling back to the pre-#298 behaviour is still the
	/// safe direction; a dialog cleared slightly early is recoverable,
	/// one left stuck for minutes is the bug this exists to fix. Unlike
	/// `releasePermission` (adapter vanished entirely) this fires on a
	/// live, healthy adapter mid-conversation, so it must do the one
	/// narrow thing it's proof of and nothing more: never touch
	/// `waiting`, `idle`, `spawning` or `exited`. A subagent tool
	/// result is not "the harness is now doing work" (it might be the
	/// main session sitting at `waiting` while a background subagent
	/// finishes up) — it is only "whatever dialog was on screen has
	/// been answered." All further phase/notification policy belongs
	/// to #277.
	clearPermission(id: string, source: TransitionSource, agentId?: string | null): void {
		const cur = store.get(id);
		if (cur?.phase !== "permission") return;
		if (cur.permissionAgentId != null && cur.permissionAgentId !== agentId) return;
		setPhase(id, "running", source);
	},

	/// #277: a subagent started LIVE (never an attach-time replay — the
	/// translator only calls this for `event.initial !== true`). Bumps
	/// `delegatedCount` (what the notification wording counts) and
	/// `delegationActivityAt` (what the Rule 4 ceiling measures
	/// silence against). Silent mutation, no emit — the way
	/// `recordOutput` bumps `lastOutputAt` — subscribers must not
	/// re-render on subagent chatter, only on the harness's own phase.
	noteSubagentStarted(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegatedCount += 1;
		cur.delegationActivityAt = Date.now();
	},

	/// #277: a subagent produced a tool result or reached its own end
	/// of turn. Bumps `delegationActivityAt` only — proof of life for
	/// the Rule 4 ceiling. Doesn't touch `delegatedCount`: that counts
	/// starts, not activity. Silent mutation, no emit, same reasoning
	/// as `noteSubagentStarted`.
	noteSubagentActivity(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegationActivityAt = Date.now();
	},

	/// #277: the user submitted a fresh prompt to this harness — resets
	/// `delegatedCount` to 0, since it counts delegations since the
	/// LAST prompt. Silent mutation, no emit, same reasoning as
	/// `noteSubagentStarted`.
	noteUserPrompt(id: string): void {
		const cur = store.get(id);
		if (!cur) return;
		cur.delegatedCount = 0;
	},

	/// Record a chunk of PTY output. Side-effects: bumps
	/// `lastOutputAt`; appends the stripped chunk to the L2b tail
	/// buffer (only when no L2c adapter is attached); transitions
	/// `spawning|idle → running` if applicable; ignored when already
	/// `exited`. Skipped entirely during an active mute window so
	/// induced redraws (focus events, resize) don't reset the idle
	/// timer.
	///
	/// When a harness has an L2c adapter attached (`authoritative`),
	/// PTY chunks still bump `lastOutputAt` for diagnostics but do
	/// NOT change phase and do NOT feed the tail buffer. The
	/// adapter is the truth source for those harnesses; running the
	/// pattern matcher on Claude / opencode TUI output would only
	/// invite false positives (an assistant message that quotes
	/// "(y/n)" shouldn't flip the dot).
	///
	/// `chunk` is optional for backwards compatibility — callers
	/// that don't have the raw bytes (e.g. synthetic recordings)
	/// can pass `undefined` and the tail buffer stays empty for
	/// that harness.
	recordOutput(id: string, chunk?: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		const now = Date.now();
		const mute = muteUntil.get(id);
		if (mute !== undefined && now < mute) return;
		if (cur.authoritative) {
			// Adapter owns phase. Update lastOutputAt silently so any
			// future detach-fallback to L2a starts with a fresh
			// timestamp; don't touch phase or tail.
			cur.lastOutputAt = now;
			return;
		}
		// #86: a harness blocked on `permission` stays blocked no
		// matter what repaints across the PTY — the dialog itself is
		// what's producing this output. Only decisive user input
		// (`recordInput`) or the adapter's own resolution signal may
		// move it; falling through to the L2b tail/pattern logic below
		// could otherwise misread the dialog's own text as a drained
		// prompt and flip back to running early.
		if (cur.phase === "permission") {
			cur.lastOutputAt = now;
			return;
		}
		// L2b: append stripped output to the tail buffer. The
		// matcher only looks at the last 256 chars, but we keep
		// 2 KB so a single fat PTY chunk doesn't immediately blow
		// past the matcher's window.
		if (chunk !== undefined && chunk.length > 0) {
			const stripped = stripAnsi(chunk);
			if (stripped.length > 0) {
				const combined = cur.tail + stripped;
				cur.tail = combined.length > TAIL_MAX_CHARS ? combined.slice(-TAIL_MAX_CHARS) : combined;
			}
		}
		// L2b: if we were in `waiting` and the freshly-arrived
		// output drained the prompt out of the tail (user
		// answered, child kept going), flip back to running.
		// Without this, "Password:" → user types → output continues
		// → we'd stay stuck in waiting because the pattern matched
		// at the moment of transition but not anymore.
		if (cur.phase === "waiting") {
			if (matchesWaitingPrompt(cur.tail)) {
				cur.lastOutputAt = now;
				return;
			}
			setPhase(id, "running", TRANSITION_SOURCE.L2bDrained, { lastOutputAt: now });
			return;
		}
		if (cur.phase === "running") {
			// Silent mutation: every PTY chunk fires this — emitting
			// would re-render every subscriber on every chunk. The
			// tick reads the latest `lastOutputAt` so detection
			// stays correct.
			cur.lastOutputAt = now;
			return;
		}
		setPhase(id, "running", TRANSITION_SOURCE.PtyOutput, { lastOutputAt: now });
	},

	/// Mute incoming output for this harness for ~800 ms. Call right
	/// before causing an action that's likely to provoke an "induced"
	/// redraw from the child — currently used by `LiveTerminal` when
	/// it forwards a focus-in/-out escape (`\x1b[I` / `\x1b[O`) to
	/// the child. Many TUIs (Claude Code, opencode) redraw their
	/// whole screen in response, which we don't want counted as the
	/// child being "active" — the child only redrew because we
	/// poked it.
	muteInducedOutput(id: string): void {
		muteUntil.set(id, Date.now() + INDUCED_MUTE_MS);
	},

	/// Record PTY exit. Transitions to `exited` regardless of prior
	/// phase. Idempotent.
	exited(id: string, code: number | null): void {
		const cur = store.get(id);
		if (!cur) {
			// A late exit for a harness we never saw spawn (shouldn't
			// normally happen but worth recording so consumers see
			// the truth). Never had a deferral armed, so the #277
			// fields are just their spawn defaults.
			store.set(id, {
				phase: "exited",
				lastOutputAt: null,
				exitCode: code,
				spawnedAt: Date.now(),
				hasUserInput: false,
				authoritative: false,
				tail: "",
				permissionTool: null,
				permissionAgentType: null,
				permissionAgentId: null,
				adapterHeard: false,
				promptSubmittedAt: null,
				adapterSilent: false,
				injected: false,
				delegationDeferredAt: null,
				delegationActivityAt: Date.now(),
				delegationEmptiedAt: null,
				delegatedCount: 0,
			});
			emit(id);
			return;
		}
		if (cur.phase === "exited") return;
		// #277 Rule 2: an exited harness can't still be waiting on its
		// subagents.
		disarmDelegation(id);
		setPhase(id, "exited", TRANSITION_SOURCE.PtyExit, { exitCode: code });
	},

	/// Drop a harness from the store entirely. Call from LiveTerminal
	/// on unmount so we don't accumulate dead entries.
	forget(id: string): void {
		if (!store.has(id) && !listeners.has(id) && !muteUntil.has(id)) return;
		const wasPermission = store.get(id)?.phase === "permission";
		store.delete(id);
		listeners.delete(id);
		muteUntil.delete(id);
		stopTickIfIdle();
		if (wasPermission) recomputePermissionIds();
	},

	get(id: string): HarnessActivity | null {
		return store.get(id) ?? null;
	},

	subscribe(id: string, cb: () => void): () => void {
		let set = listeners.get(id);
		if (!set) {
			set = new Set();
			listeners.set(id, set);
		}
		set.add(cb);
		return () => {
			const s = listeners.get(id);
			if (!s) return;
			s.delete(cb);
			if (s.size === 0) listeners.delete(id);
		};
	},

	/// Subscribe to every phase transition across every harness.
	/// One callback fires for each real transition with `(id, from,
	/// to)`. Returns an unsubscribe. Used by App-level notification
	/// logic that fans out to multiple rooms.
	subscribeTransitions(cb: TransitionListener): () => void {
		transitionListeners.add(cb);
		return () => {
			transitionListeners.delete(cb);
		};
	},
};

/// React hook: subscribes to the activity store for a single
/// harness and returns its current state. Returns `null` for
/// unknown ids (e.g. archived harnesses) so callers can fall
/// back gracefully.
export function useHarnessActivity(id: string | null): HarnessActivity | null {
	return useSyncExternalStore(
		(cb) => {
			if (id === null) return () => {};
			return harnessActivity.subscribe(id, cb);
		},
		() => (id === null ? null : harnessActivity.get(id)),
	);
}

/// React hook: every harness id currently in `permission`, across
/// every room. Backs the status-bar urgent slot (#86), which needs to
/// know whether *anything* is blocked on a dialog without subscribing
/// to each harness in every room individually. The returned Set is
/// referentially stable across renders that don't change membership,
/// as `useSyncExternalStore` requires.
export function usePermissionHarnessIds(): ReadonlySet<string> {
	return useSyncExternalStore(
		(cb) => {
			permissionListeners.add(cb);
			return () => {
				permissionListeners.delete(cb);
			};
		},
		() => permissionIds,
	);
}

/// Map the internal activity phase onto the existing display
/// `Status` enum used by `StatusDot` and the status bar. Keep
/// the mapping centralized so future strategies (additional L2c
/// adapters, eventual L2b pattern fallback) refine it in one place.
export function activityToStatus(activity: HarnessActivity | null): Status {
	if (!activity) return "running";
	switch (activity.phase) {
		case "spawning":
		case "running":
			return "running";
		case "idle":
			return "idle";
		case "waiting":
			return "waiting";
		case "permission":
			return "permission";
		case "exited":
			return "exited";
	}
}

/// Human-facing text for a `Status`, for the surfaces that print the
/// raw word (bottom status bar, the hover popover). `permission` reads
/// as "permission needed" — never the bare word "permission", which
/// reads as a noun with no verb and doesn't say what's expected of the
/// user — plus the tool name when the adapter could say (#86), plus
/// the subagent name when the dialog belongs to one rather than the
/// main session (#298), e.g. "permission needed · explore · Bash".
/// Both existing shapes (no tool, tool only) are unchanged when
/// `permissionAgentType` is absent or `null`.
///
/// #277: `running` with a non-zero `workingSubagentCount` reads as
/// "delegating · N agents" ("1 agent" singular) instead of the bare
/// "running" — the phase itself doesn't change (the dot stays green
/// via `effectiveStatus`), only the word this function prints. Any
/// other status ignores the count entirely, `permission` included —
/// a harness can't be both blocked on a dialog and shown as
/// delegating.
export function statusLabel(
	status: Status,
	permissionTool?: string | null,
	permissionAgentType?: string | null,
	workingSubagentCount?: number,
): string {
	if (status === "permission") {
		const parts = [permissionAgentType, permissionTool].filter(
			(p): p is string => p !== null && p !== undefined,
		);
		return parts.length > 0 ? `permission needed · ${parts.join(" · ")}` : "permission needed";
	}
	if (status === "running" && workingSubagentCount) {
		return `delegating · ${workingSubagentCount} agent${workingSubagentCount === 1 ? "" : "s"}`;
	}
	return status;
}

/// #277: the notification-wording half of the deferred end-of-turn —
/// how many subagents this harness delegated since the user's last
/// prompt (`HarnessActivity.delegatedCount`), worded for a toast
/// subtitle / OS banner suffix. `null` for zero, so call sites can
/// `if (summary)` rather than check the count themselves. Pure and
/// exported so the wording is unit-testable without going through
/// App.tsx's notification effect.
export function delegationSummary(count: number): string | null {
	if (count <= 0) return null;
	return count === 1 ? "1 delegated agent finished" : `${count} delegated agents finished`;
}

/// `activityToStatus` with the "acknowledged" downgrade applied:
/// when the phase is `waiting` and the harness has zero pending
/// notifications, render as `idle` (grey) instead of `waiting`
/// (blue + pulse). The user has already been to the harness
/// since the last transition; the dot's job there is "telling you
/// what's new," and there's nothing new to tell.
///
/// The bottom-bar TEXT label still uses the underlying phase
/// (`waiting`) so callers can honestly report what Claude is
/// doing; only the visual indicator collapses.
export function effectiveStatus(
	activity: HarnessActivity | null,
	pendingNotifications: number,
): Status {
	const base = activityToStatus(activity);
	if (base === "waiting" && pendingNotifications === 0) return "idle";
	return base;
}

/// Priority order for combining multiple harness statuses into a
/// single room-level status (epic #50 L4). Higher = more important
/// to surface on the room dot. `permission` lands top (#86) — a
/// harness blocked on an approval dialog is a harder stop than one
/// merely waiting for the user's next prompt.
const STATUS_PRIORITY: Record<Status, number> = {
	permission: 6,
	waiting: 5,
	running: 4,
	idle: 3,
	exited: 2,
	error: 1,
};

/// Minimal shape `useRoomActivity` needs from each harness: an id
/// for store lookup + the pending-notifications count for the
/// effective-status downgrade. `pendingNotifications` is explicitly
/// `| undefined` so it accepts the `Harness` type (where the field
/// is optional) under `exactOptionalPropertyTypes`.
export interface RoomHarnessRef {
	id: string;
	pendingNotifications?: number | undefined;
}

/// Pick whichever of two statuses is more important to surface, per
/// `STATUS_PRIORITY`. Exported so the priority ordering itself — e.g.
/// "permission outranks waiting" (#86) — is unit-testable directly,
/// without going through `useRoomActivity`, which needs a React
/// render context this project's node-environment test suite doesn't
/// have. Ties keep `a`, matching the previous inline `>` comparison
/// this replaced (first-seen wins a tie).
export function higherPriorityStatus(a: Status, b: Status): Status {
	return STATUS_PRIORITY[b] > STATUS_PRIORITY[a] ? b : a;
}

/// Combine a room's harnesses into one room-level status. `lookup`
/// is injected (rather than reading the module-level `store`
/// directly) so this is unit-testable without React or the store's
/// global state — see `harnessActivity.test.ts`.
///
/// Returns `"idle"`, never `null`, when no harness in the list has a
/// record yet (e.g. a room made up only of non-PTY harnesses like
/// `files`, or the first paint before any LiveTerminal effect has
/// fired) — issue #290: a room's status derives ONLY from its
/// harnesses, so "nothing has reported in" reads as idle rather than
/// falling back to the room's stale persisted `status` field.
export function aggregateRoomStatus(
	harnesses: readonly RoomHarnessRef[],
	lookup: (id: string) => HarnessActivity | null | undefined,
): Status {
	let best: Status | null = null;
	for (const h of harnesses) {
		const a = lookup(h.id);
		if (!a) continue;
		// Per-harness effective status: waiting downgrades to idle
		// when the harness has been viewed since the last
		// transition. Otherwise a room with one waiting-but-
		// acknowledged harness would keep pulsing the room tab
		// even though the user knows.
		const s = effectiveStatus(a, h.pendingNotifications ?? 0);
		best = best === null ? s : higherPriorityStatus(best, s);
	}
	return best ?? "idle";
}

/// React hook: subscribes to every harness in a room and returns
/// the aggregate room status via `aggregateRoomStatus`. Always
/// returns a concrete `Status` — `"idle"` when no harness in the
/// list has a record yet — so callers use it directly and never fall
/// back to the persisted `room.status` (#290).
///
/// Takes the full harness records (rather than just ids) so the
/// aggregation can apply the same acknowledged-downgrade rule
/// `effectiveStatus` uses per-harness — a room dot shouldn't pulse
/// for a harness the user has already seen.
///
/// Caller responsibility: pass a stable `harnesses` reference
/// (use useMemo). `useSyncExternalStore` re-subscribes whenever
/// `subscribe` changes; a fresh array reference each render would
/// thrash the listener Sets without changing behaviour.
export function useRoomActivity(harnesses: readonly RoomHarnessRef[]): Status {
	const subscribe = useCallback(
		(cb: () => void) => {
			const unsubs = harnesses.map((h) => harnessActivity.subscribe(h.id, cb));
			return () => {
				for (const u of unsubs) u();
			};
		},
		[harnesses],
	);
	const getSnapshot = useCallback(
		() => aggregateRoomStatus(harnesses, (id) => store.get(id)),
		[harnesses],
	);
	return useSyncExternalStore(subscribe, getSnapshot);
}
