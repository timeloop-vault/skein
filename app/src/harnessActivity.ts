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
//
// #19: this module used to be one ~1450-line file. It is now the
// entry point — the `harnessActivity` store object plus re-exports —
// with the pieces that don't need to share its closure split out:
// harnessActivityTypes.ts (ActivityPhase/HarnessActivity/TransitionSource
// /TRANSITION_SOURCE), harnessActivityConstants.ts (the tuning
// thresholds), harnessActivityCore.ts (the mutable store state, setPhase,
// the background tick, the #259/#273 watchdog degrade helpers, #277's
// disarmDelegation, isDecisiveInput), harnessActivityLabels.ts (pure
// status/label derivation) and useHarnessActivity.ts (the React hooks).
// Every symbol this module exported before the split is still exported
// from here, so `import { ... } from "./harnessActivity.ts"` is unchanged
// for every caller.

import { INDUCED_MUTE_MS, TAIL_MAX_CHARS } from "./harnessActivityConstants.ts";
import {
	delegationDeferredListeners,
	disarmDelegation,
	emit,
	ensureTick,
	isDecisiveInput,
	listeners,
	muteUntil,
	phaseSnapshot,
	recomputePermissionIds,
	setPhase,
	shouldArmWatchdog,
	stopTickIfIdle,
	store,
	transitionListeners,
} from "./harnessActivityCore.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import type {
	ActivityPhase,
	HarnessActivity,
	TransitionListener,
	TransitionSource,
} from "./harnessActivityTypes.ts";
import { stripAnsi } from "./harnessPatterns.ts";
import { matchesWaitingPrompt } from "./harnessPatterns.ts";
import { subagents } from "./subagents.ts";

export type {
	ActivityPhase,
	HarnessActivity,
	TransitionListener,
	TransitionSource,
} from "./harnessActivityTypes.ts";
export { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
export { atSafeStoppingPoint, isDecisiveInput, phaseSnapshot } from "./harnessActivityCore.ts";
export {
	activityToStatus,
	aggregateRoomStatus,
	delegationSummary,
	effectiveStatus,
	higherPriorityStatus,
	statusLabel,
} from "./harnessActivityLabels.ts";
export type { RoomHarnessRef } from "./harnessActivityLabels.ts";
export {
	useHarnessActivity,
	usePermissionHarnessIds,
	useRoomActivity,
} from "./useHarnessActivity.ts";

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
			degradedBy: null,
			launchSignalAt: null,
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
			degradedBy: null,
			...(cur.adapterSilent ? { authoritative: true } : null),
		});
	},

	/// #273: the harness's own CLI reported its own launch (Claude's
	/// `SessionStart` hook, relayed via `skein://harness-session-start`).
	/// No-op for an unknown harness or one already `exited`.
	///
	/// Always records `launchSignalAt` (even on a second call — Claude
	/// is known to fire `SessionStart` twice for a "phantom" session
	/// within a few hundred ms, anthropics/claude-code#78455 — so this
	/// must be idempotent and carry no consume-once side effect; a
	/// re-record is harmless since nothing here treats it as an edge).
	///
	/// Only the launch-silent watchdog's diagnosis is voided by this
	/// event — `recovering` below checks `degradedBy`, not just
	/// `adapterSilent`. A launch signal means the harness IS alive and
	/// sitting at its prompt, which is exactly what
	/// `degradeLaunchSilentAdapter` had no proof of; it says nothing at
	/// all about what `degradeSilentAdapter` (#259) diagnosed — a
	/// PROMPTED tail that stayed mute, which usually means the tail is
	/// watching the wrong file entirely. A `SessionStart` hook firing
	/// proves nothing about that, so an adapter-silent harness must
	/// stay exactly as the #259 watchdog left it: `authoritative`
	/// false, `adapterSilent` true, phase untouched. Only
	/// `adapterDelivered` — the tail actually speaking — may recover
	/// that one. `launchSignalAt` is still recorded either way; that
	/// fact is true regardless of which watchdog fired.
	///
	/// When it does recover (the launch-silent case), it restores the
	/// harness exactly the way `adapterDelivered` does — authority
	/// back, `adapterSilent` and `degradedBy` cleared, same
	/// `console.info` recovery line — the "accepted the trust dialog
	/// late" case.
	///
	/// Moves phase to `waiting` only when `!cur.adapterHeard` AND the
	/// phase is `spawning` or the harness had just been recovered from
	/// `adapterSilent`. Never while `phase === "permission"` (#86
	/// outranks everything). The `!cur.adapterHeard` guard is load-
	/// bearing, not incidental: the hook entry is matcher-less, so it
	/// fires for every `SessionStart` source — `startup`, but also
	/// `resume`/`clear`/`compact`/`fork` mid-session — and `source`
	/// is deliberately not on the event payload, so this guard is the
	/// only thing telling a genuine launch apart from a `clear`/
	/// `compact` firing mid-conversation: by the time either of those
	/// happens, the transcript tail has always already spoken, and a
	/// live adapter is the better authority (it may correctly know the
	/// session is mid-turn, which this event can't say either way).
	/// A second `noteLaunchSignal` while already `waiting` is a no-op
	/// by construction — `setPhase`'s same-phase short-circuit.
	noteLaunchSignal(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited") return;
		const recovering = cur.adapterSilent && cur.degradedBy === "launch-silent";
		if (recovering) {
			console.info(`[skein] harness ${id}: launch signal arrived; handing phase back to it`);
		}
		store.set(id, {
			...cur,
			launchSignalAt: Date.now(),
			...(recovering ? { adapterSilent: false, degradedBy: null, authoritative: true } : null),
		});
		if (cur.phase === "permission") return;
		if (!cur.adapterHeard && (cur.phase === "spawning" || recovering)) {
			setPhase(id, "waiting", TRANSITION_SOURCE.L2c1ClaudeSessionStart);
		}
	},

	/// #116: `/clear` or an in-tool `/resume` re-points the JSONL tail
	/// onto a different session id mid-process — see
	/// `sessionTracking.ts`'s `followedSession`, which decides *whether*
	/// to follow; this is what the harness's state needs once the
	/// caller has decided to. `source` only picks which
	/// `TRANSITION_SOURCE` the phase move is attributed to; the rest of
	/// the handling is identical for both.
	///
	/// The subagent registry is keyed by harness, not session, so a
	/// subagent still tracked as live under the old session would
	/// otherwise keep `subagents.workingCount` non-zero forever — the
	/// new session's own tail never reports its end, since it never
	/// started it. Forgetting here means the new session starts clean;
	/// disarming any armed #277 deferral for the same reason
	/// `detachAuthoritativeSource` does — the old session's
	/// `awaiting_prompt` that would have resolved it is never coming.
	///
	/// If the phase is `running`, move it to `waiting`: by the time
	/// this fires, the picker has already returned Claude to its prompt
	/// — for `/clear` that's because nothing is written to the
	/// transcript for `/clear` itself (sampled: no row); for `/resume`
	/// it's because the picker only returns once a conversation has been
	/// chosen and reattached. Either way no ordinary end-of-turn event
	/// is coming to do this move, and the re-attach's own initial event
	/// (LiveTerminal, keyed off the `sessionId` prop) then reflects the
	/// resumed/cleared transcript from here on. Every other phase is
	/// left alone: `permission` outranks it (#86), and `exited` /
	/// `spawning` / `idle` / `waiting` aren't this event's business.
	/// No-op for an unknown id.
	sessionSwitched(id: string, source: "clear" | "resume" | "fork"): void {
		const cur = store.get(id);
		if (!cur) return;
		subagents.forget(id);
		disarmDelegation(id);
		if (cur.phase !== "running") return;
		setPhase(
			id,
			"waiting",
			source === "clear"
				? TRANSITION_SOURCE.L2c1ClaudeSessionClear
				: source === "resume"
					? TRANSITION_SOURCE.L2c1ClaudeSessionResume
					: TRANSITION_SOURCE.L2c1ClaudeSessionFork,
		);
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
			// #381: notify arm-only listeners — see
			// `subscribeDelegationDeferred` below. Must sit inside this
			// guard, not after it, so the idempotent re-arm case (a second
			// end-of-turn while already deferred) never fires it twice.
			for (const cb of delegationDeferredListeners) cb(id);
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
		const armWatchdog = shouldArmWatchdog(cur) && (data.includes("\r") || data.includes("\n"));
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

	/// #363: arm the #259 silent-adapter watchdog for a prompt submitted
	/// through the `sendPrompt` seam — `create_room`'s first prompt, a
	/// mail nudge, anything that pastes and submits without ever
	/// touching xterm's `onKey` and so never reaches `recordInput` above.
	/// Shares `recordInput`'s arm guard (`shouldArmWatchdog`, factored
	/// into `harnessActivityCore.ts` so the two can't drift apart) minus
	/// the "does `data` contain a newline" check — a `sendPrompt` call
	/// IS the submit, unconditionally, the same as a typed Enter.
	///
	/// Deliberately narrow: it only arms the timer. Doesn't touch
	/// `hasUserInput` — that flag means a human typed something, and a
	/// programmatic submit isn't that. Refuses to arm at all while
	/// `phase === "permission"` — an armed timer would eventually strip
	/// authority (`degradeSilentAdapter`) out from under a harness whose
	/// dialog is simply still open, not silent — and never clears
	/// `permission` or moves phase either way: #86 established that only
	/// a decisive user keystroke is proof a dialog was answered, and a
	/// programmatic submit is not one. `canSendPrompt` already refuses to
	/// send into `permission`, but this method doesn't lean on that; it
	/// makes the same call on its own terms, independently, since nothing
	/// stops a future caller from invoking it directly. No emit, matching
	/// `recordInput`'s own arm path — `promptSubmittedAt` is bookkeeping
	/// the tick reads, not a phase change any subscriber needs to hear
	/// about.
	notePromptSubmitted(id: string): void {
		const cur = store.get(id);
		if (!cur || cur.phase === "permission" || !shouldArmWatchdog(cur)) return;
		store.set(id, { ...cur, promptSubmittedAt: Date.now() });
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
				degradedBy: null,
				launchSignalAt: null,
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

	/// #356: every harness id this process has a phase for, for the
	/// `harness_phases` agent-request kind. See `phaseSnapshot`'s own
	/// doc for why an id can be missing from the result.
	phaseSnapshot(): Record<string, ActivityPhase> {
		return phaseSnapshot(store);
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

	/// #381: subscribe to #277 delegation deferrals ARMING — `cb(id)`
	/// fires the instant a harness's end-of-turn is deferred for
	/// background subagents (`awaitingPromptFromAdapter`'s
	/// `delegationDeferredAt` going null → non-null), never on the
	/// idempotent re-arm and never on disarm. Step B (mail delivery
	/// during a deferral) is the intended caller — see
	/// `atSafeStoppingPoint` for the companion "is it still safe to send
	/// into right now" check. Returns an unsubscribe, same shape as
	/// `subscribeTransitions`.
	subscribeDelegationDeferred(cb: (id: string) => void): () => void {
		delegationDeferredListeners.add(cb);
		return () => {
			delegationDeferredListeners.delete(cb);
		};
	},
};
