// harnessActivity.ts split (#458): silence recovery and the #423 supervisor repairs. Methods are spread into the
// `harnessActivity` store object there; shared state lives in harnessActivityCore.ts.

import { logBoth } from "./frontendLog.ts";
import { setPhase, store } from "./harnessActivityCore.ts";
import { TRANSITION_SOURCE } from "./harnessActivityTypes.ts";

export const supervisorMethods = {
	/// #404: undo a #259 adapter-silent degrade that was actually caused
	/// by a mail nudge (or any #238 seam submit) whose own submit never
	/// landed — #388's settlement rolls that nudge back, but by then the
	/// watchdog has already stripped authority from a harness that was
	/// never given a real prompt to answer, and nothing else would ever
	/// call `adapterDelivered` for it again. `useMailDelivery.ts`'s
	/// rollback branch is the only caller, and only once it has also
	/// confirmed no human drove the harness in the meantime
	/// (`mailSettle.ts`'s `shouldRecoverSilence`) — a keystroke since the
	/// nudge is evidence this diagnosis might be wrong.
	///
	/// Recovers only when every one of these holds — deliberately as
	/// narrow as `noteLaunchSignal`'s own recovery guard, since restoring
	/// authority on weak evidence reintroduces exactly the false
	/// "watching" reading #259 exists to prevent:
	///
	///  - the harness still exists and isn't `exited` — nothing to
	///    recover into; or `permission` — #86 outranks this everywhere
	///    else this interaction shows up, and this is no exception;
	///  - `adapterSilent && degradedBy === "adapter-silent"` — the #259
	///    watchdog is what fired, not #273's launch-silent timer. A
	///    harness that degraded before it ever got a launch signal has no
	///    "lost first paste" story to recover;
	///  - `!adapterHeard` — the tail never spoke at all since spawn, so
	///    nothing but this one lost prompt explains the silence. A tail
	///    that DID speak and later went silent is a different, real
	///    failure this must not paper over;
	///  - `launchSignalAt !== null` — the CLI proved it was alive at its
	///    own prompt, the same proof `canSendPrompt` accepted to let the
	///    nudge be sent in the first place;
	///  - `!silenceRecovered` — once per spawn. A second silent seam
	///    submit on the same spawn, after a recovery already happened, is
	///    real evidence the tail is watching the wrong file, not another
	///    lost race — it must stay degraded.
	///
	/// Effect, mirroring `adapterDelivered`/`noteLaunchSignal`'s own
	/// recovery paths: `authoritative` back to `true`, `adapterSilent`
	/// and `degradedBy` cleared, plus `promptSubmittedAt` cleared so the
	/// very next seam submit re-arms the watchdog from a clean slate
	/// (rather than reading as already-armed against a timestamp from the
	/// lost attempt), and `silenceRecovered` set so this can't fire twice.
	/// Moves phase to `waiting` when it isn't already there — the launch
	/// signal already proved the CLI is sitting at its own prompt.
	/// Returns whether it actually recovered, so the caller can log the
	/// outcome and gate a retry on it.
	///
	/// "Recovered" means only that the gate is reopened — `authoritative`
	/// back to `true` — never that the harness is actually proven idle at
	/// its prompt. If the diagnosis is wrong (a broken tail, #362, that
	/// was never going to report anything regardless of whether the nudge
	/// landed) this lets one extra automatic nudge through per spawn,
	/// bounded by `silenceRecovered` above, and `decideMailNudge` already
	/// dedupes on the live unread count once the agent actually reads its
	/// mail — so a wrong recovery costs at most one stray paste, not a
	/// runaway. That is the accepted trade-off against the alternative:
	/// a harness stuck permanently refused because its one real proof of
	/// life was the lost nudge this exists to recover.
	recoverUnheardSilence(id: string): boolean {
		const cur = store.get(id);
		if (!cur || cur.phase === "exited" || cur.phase === "permission") return false;
		if (!cur.adapterSilent || cur.degradedBy !== "adapter-silent") return false;
		if (cur.adapterHeard || cur.launchSignalAt === null) return false;
		if (cur.silenceRecovered) return false;
		logBoth(
			"info",
			"skein::activity",
			`[skein] harness ${id}: recovering from a lost first paste — the silence was never the adapter's fault (#404)`,
		);
		store.set(id, {
			...cur,
			authoritative: true,
			adapterSilent: false,
			degradedBy: null,
			promptSubmittedAt: null,
			silenceRecovered: true,
		});
		if (store.get(id)?.phase !== "waiting") {
			setPhase(id, "waiting", TRANSITION_SOURCE.SilenceRecovered);
		}
		return true;
	},

	/// #423: the supervisor's authority repair — an adapter that is
	/// speaking again but was never handed authority back. Guards are
	/// re-checked here, at call time, not trusted from the caller's earlier
	/// read: the harness exists, authority is really gone, no watchdog
	/// diagnosis (`adapterSilent`) is standing — that recovery belongs to
	/// `adapterDelivered` — and it hasn't exited. Changes no phase. Returns
	/// whether it applied.
	supervisorRegrantAuthority(id: string): boolean {
		const cur = store.get(id);
		if (!cur || cur.authoritative || cur.adapterSilent || cur.phase === "exited") return false;
		logBoth(
			"info",
			"skein::activity",
			`[skein] harness ${id}: supervisor restoring adapter authority (#423)`,
		);
		store.set(id, { ...cur, authoritative: true });
		return true;
	},

	/// #423: the supervisor's phase repair — a harness whose end of turn
	/// was seen but whose phase never followed. Only from `running` or
	/// `idle`, and never while a #277 deferral is armed (that harness is
	/// legitimately `running` until its subagents finish). `permission`,
	/// `exited`, `spawning` and `waiting` are left alone. Guards are
	/// re-checked at call time. Returns whether it applied.
	supervisorSetWaiting(id: string): boolean {
		const cur = store.get(id);
		if (!cur || (cur.phase !== "running" && cur.phase !== "idle")) return false;
		if (cur.delegationDeferredAt !== null) return false;
		setPhase(id, "waiting", TRANSITION_SOURCE.SupervisorRecovered);
		return true;
	},
};
