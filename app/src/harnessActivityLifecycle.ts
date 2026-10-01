// harnessActivity.ts split (#458): harness lifecycle and adapter authority. Methods are spread into the
// `harnessActivity` store object there; shared state lives in harnessActivityCore.ts.

import { backgroundTasks } from "./backgroundTasks.ts";
import { logBoth } from "./frontendLog.ts";
import {
	disarmDelegation,
	emit,
	ensureTick,
	listeners,
	muteUntil,
	recomputePermissionIds,
	setPhase,
	stopTickIfIdle,
	store,
} from "./harnessActivityCore.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";
import { subagents } from "./subagents.ts";

export const lifecycleMethods = {
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
			silenceRecovered: false,
			phaseSince: now,
			lastTurnSignal: null,
			lastAdapterEvent: null,
			authorityLostAt: null,
			permissionAt: null,
			lastSubmitAt: null,
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
	///
	/// #422: an adapter that speaks while `authoritative` is false and
	/// no watchdog fired (its own `session_end` detached it, and the
	/// transcript then reappeared on the same channel) takes authority
	/// back too. Pass `restoresAuthority: false` for the event that
	/// itself announces the adapter's loss (`session_end`).
	adapterDelivered(id: string, opts?: { restoresAuthority?: boolean }): void {
		const cur = store.get(id);
		if (!cur) return;
		const restores = opts?.restoresAuthority ?? true;
		// #423: stamped on every call, ahead of the early return below.
		cur.lastAdapterEvent = { at: Date.now(), restoresAuthority: restores };
		const regrant = restores && !cur.authoritative && !cur.adapterSilent && cur.phase !== "exited";
		if (cur.adapterHeard && !cur.adapterSilent && !regrant) return;
		if (cur.adapterSilent) {
			logBoth(
				"info",
				"skein::activity",
				`[skein] harness ${id}: adapter recovered; handing phase back to it`,
			);
		} else if (regrant) {
			logBoth(
				"info",
				"skein::activity",
				`[skein] harness ${id}: adapter delivered while not authoritative; restoring its authority (#422)`,
			);
		}
		store.set(id, {
			...cur,
			adapterHeard: true,
			adapterSilent: false,
			degradedBy: null,
			...(cur.adapterSilent || regrant ? { authoritative: true } : null),
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
			logBoth(
				"info",
				"skein::activity",
				`[skein] harness ${id}: launch signal arrived; handing phase back to it`,
			);
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
		backgroundTasks.forget(id);
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
		store.set(id, { ...cur, authoritative: false, authorityLostAt: Date.now() });
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
				silenceRecovered: false,
				phaseSince: Date.now(),
				lastTurnSignal: null,
				lastAdapterEvent: null,
				authorityLostAt: null,
				permissionAt: null,
				lastSubmitAt: null,
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
};
