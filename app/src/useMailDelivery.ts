// #329: delivers agent mail into a waiting harness by pasting a nudge
// through the #238 seam (`harnessInput.sendPrompt`), and keeps
// `mailStore.ts`'s per-harness unread marker fresh. Called once from
// App.tsx with the current (non-archived) rooms — it owns no UI of its
// own; the tab marker lives in `components.tsx`'s `HarnessTab` via
// `useUnreadMail`.
//
// Five event triggers run the same `check(harnessId)`:
//
//   - `skein://mail-changed` for a harness this hook knows about — a
//     message just landed for it, or it just read some of its own
//     (either way the count may have moved);
//   - a phase transition that lands on, or reveals, a safe stopping
//     point (`harnessActivity.subscribeTransitions`, filtered by
//     `shouldCheckOnTransition` — #381): the plain `→ waiting` case,
//     plus `permission → running` while a #277 delegation deferral is
//     still armed, since closing the dialog can itself be the event
//     that re-enters the deferred state with no `waiting` transition
//     at all;
//   - a #277 delegation deferral ARMING with no phase change at all
//     (`harnessActivity.subscribeDelegationDeferred`) — the harness was
//     already `running` and stays `running`, so nothing above would
//     otherwise fire;
//   - #383: a harness's composer draft CLEARING with no submit
//     (`harnessInput.subscribeDraftCleared`) — the only way held mail
//     gets retried once the thing holding it goes away without also
//     firing a phase transition. A draft cleared BY a submit is not
//     this trigger's job: the turn that submit starts is itself the
//     next trigger, via the existing running → waiting transition
//     above;
//   - #386: a harness finishing registration in the #238 seam
//     (`harnessInput.subscribeRegistered`) — closes the bug that
//     motivated this issue: mail arrives while a fresh harness is still
//     `spawning`, its one `spawning → waiting` transition is missed or
//     itself refused because the seam hasn't registered yet, and this
//     is what gives it another chance the moment it does.
//
// #386: that list is deliberately not treated as exhaustive — a refused
// check can become safe for a reason no event announces. So any `check`
// that leaves mail pending (`mailRetry.ts`'s `mailPending`) arms a
// bounded retry: re-running the exact same `check` — never a looser
// gate, only a more frequent one — every `MAIL_RETRY_INTERVAL_MS` for
// up to `MAIL_RETRY_WINDOW_MS`, until it stops being pending or the
// window runs out. An event trigger re-arms the window from now; a
// retry tick that's still pending keeps the window it was already on.
// `mailRetry.ts`'s `nextMailRetry` is the pure decision underneath.
//
// #388: `sendPrompt`'s `result.ok` only proves the gate passed and the
// body was pasted — the submit itself lands later, off a timer, and can
// be silently skipped (harnessInput.ts's #380 gap-then-retry). A nudge
// therefore isn't counted as delivered the moment it's pasted: `check`
// records it PROVISIONALLY, then a settlement (`mailSettle.ts`) watches
// for proof the submit landed over `NUDGE_SETTLE_MS`, and either drops
// the provisional record (confirmed) or rolls `lastNudged` back to its
// pre-nudge value (no proof by the deadline) so the next check tries
// again — safe because `decideMailNudge` dedupes on the live unread
// count, not on `lastNudged` alone.
//
// Every trigger and every retry tick goes through `runSerialized` per
// harness, so none of them can run concurrently or double-nudge —
// `decideMailNudge` itself is pure and stateless per call, so the
// ordering has to be enforced here.
//
// Restart replay: `lastNudgedRef` starts empty every mount, which reads
// as "never nudged" (mailNudge.ts's own documented restart case) — a
// harness with mail already stored nudges once it next reaches
// `waiting`. Seeding (on mount, and whenever a harness first appears in
// `rooms`) only refreshes the marker via `mailStore.set` and never
// calls `decideMailNudge` — a harness already sitting at `waiting` with
// mail from before Skein started does not get a synthetic nudge on
// attach, the same exclusion `useHarnessNotifications.ts` makes for a
// pre-existing `waiting` state (the `spawning → waiting` gate on its
// badge/toast/OS listener).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { logBoth } from "./frontendLog.ts";
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "./harnessInput.ts";
import type { GateResult } from "./harnessInput.ts";
import {
	automaticGate,
	decideMailNudge,
	mailNudgeText,
	shouldCheckOnTransition,
} from "./mailNudge.ts";
import { mailPending, nextMailRetry } from "./mailRetry.ts";
import { NUDGE_SETTLE_MS, evaluateSettlement } from "./mailSettle.ts";
import { mailStore } from "./mailStore.ts";
import type { HarnessKind, Room } from "./types.ts";

/// DTO mirror of `agent_api::verbs::MailUnread` — see `mail_unread` in
/// `app/src-tauri/src/agent_api/commands.rs`.
interface MailUnread {
	count: number;
	fromRoomNames: string[];
}

interface HarnessMeta {
	roomId: string;
	kind: HarnessKind;
}

export function useMailDelivery(rooms: readonly Room[]): void {
	// harnessId → {roomId, kind}, rebuilt from `rooms` on every change —
	// a ref so the event/transition listeners (mounted once, empty
	// deps) always see the latest membership without re-subscribing.
	const metaRef = useRef<Map<string, HarnessMeta>>(new Map());
	// Which harnesses have already been seeded once this session, so a
	// `rooms` change doesn't re-seed a harness this hook already knows.
	const seededRef = useRef<Set<string>>(new Set());
	// Unread count as of the last nudge actually sent, per harness.
	// Empty at mount = restart replay, per mailNudge.ts.
	const lastNudgedRef = useRef<Map<string, number>>(new Map());
	// Per-harness promise chain so a mail-changed event and a waiting
	// transition arriving together run `check` one at a time, not
	// concurrently. Resolves to whether mail is still pending after that
	// run, which is what schedules (or stops) the #386 retry below.
	const inFlightRef = useRef<Map<string, Promise<boolean>>>(new Map());
	// #386: per-harness bounded-retry state — when this harness's retry
	// window was last (re-)armed, and the pending timer for its next
	// tick, if any.
	const retryRef = useRef<
		Map<string, { armedAtMs: number; timer: ReturnType<typeof setTimeout> | null }>
	>(new Map());
	// #388: per-harness settlement state for a nudge that was just
	// pasted, but not yet proven to have been submitted — see the file
	// header and `mailSettle.ts`. `preNudge` is the `lastNudged` value
	// from BEFORE this nudge, restored on rollback; `sentAtMs` is when
	// the paste happened; `deferredAtSend`/`turnStarted` are the same
	// evidence `submitRetry.ts` uses, gathered here instead of there
	// because this settlement outlives any one `sendPrompt` call.
	const settleRef = useRef<
		Map<
			string,
			{
				preNudge: number;
				sentAtMs: number;
				deferredAtSend: number | null;
				turnStarted: boolean;
				/// #404: `harnessInput.userInputCount(id)` snapshotted at
				/// paste time — compared again at rollback to tell whether a
				/// human drove the harness in between, which
				/// `shouldRecoverSilence` needs to decide whether a
				/// lost-silence recovery is safe to attempt.
				inputCountAtSend: number;
				timer: ReturnType<typeof setTimeout>;
			}
		>
	>(new Map());
	// False once the hook has unmounted, so a `check` still in flight at
	// that moment can't schedule a timer the unmount cleanup already missed.
	const mountedRef = useRef(true);
	// #404: per-harness last-logged gate/sendPrompt refusal reason, so the
	// #386 retry tick (every `MAIL_RETRY_INTERVAL_MS`, 2 s) doesn't spam
	// `skein.log` with the same unchanging reason on every tick — only a
	// CHANGE in reason is worth another line. Cleared once a nudge goes
	// out (the reason it was tracking no longer applies) or the harness
	// is forgotten.
	const lastMailRefusalReasonRef = useRef<Map<string, string>>(new Map());

	const logMailRefusal = (harnessId: string, reason: string): void => {
		if (lastMailRefusalReasonRef.current.get(harnessId) === reason) return;
		lastMailRefusalReasonRef.current.set(harnessId, reason);
		logBoth(
			"info",
			"skein::mail",
			`[skein] useMailDelivery: harness ${harnessId} nudge refused — ${reason} (#404)`,
		);
	};

	// #404: `mailPending`'s own verdict for this run, wrapping the throttle
	// clear so a harness that's caught up (no mail left to retry for) drops
	// its remembered reason — a LATER refusal for a fresh batch of mail
	// logs again even if it happens to be the same reason as last time,
	// rather than reading as still-throttled from mail that's long gone.
	const pendingResult = (harnessId: string, count: number, lastNudged: number): boolean => {
		const pending = mailPending(count, lastNudged);
		if (!pending) lastMailRefusalReasonRef.current.delete(harnessId);
		return pending;
	};

	const clearRetry = (harnessId: string): void => {
		const entry = retryRef.current.get(harnessId);
		if (entry?.timer) clearTimeout(entry.timer);
		retryRef.current.delete(harnessId);
	};

	const clearSettlement = (harnessId: string): void => {
		const entry = settleRef.current.get(harnessId);
		if (entry) clearTimeout(entry.timer);
		settleRef.current.delete(harnessId);
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: clearRetry/clearSettlement close only over refs, stable across renders.
	useEffect(() => {
		const meta = metaRef.current;
		const seen = new Set<string>();
		for (const room of rooms) {
			for (const h of room.harnesses) {
				seen.add(h.id);
				meta.set(h.id, { roomId: room.id, kind: h.kind });
			}
		}
		// Drop meta (and seed/nudge memory) for harnesses that are gone —
		// closed, or the room archived — so nothing later drives a check
		// off a stale roomId/kind, and a harness with the same id never
		// resurfaces (ids are fresh per spawn).
		for (const id of [...meta.keys()]) {
			if (seen.has(id)) continue;
			meta.delete(id);
			seededRef.current.delete(id);
			lastNudgedRef.current.delete(id);
			lastMailRefusalReasonRef.current.delete(id);
			mailStore.forget(id);
			clearRetry(id);
			clearSettlement(id);
		}
		for (const id of seen) {
			if (seededRef.current.has(id)) continue;
			seededRef.current.add(id);
			const m = meta.get(id);
			if (!m) continue;
			invoke<MailUnread>("mail_unread", { roomId: m.roomId, harnessId: id })
				.then((res) => {
					mailStore.set(id, res.count, res.fromRoomNames);
				})
				.catch((err: unknown) => {
					logBoth(
						"warn",
						"skein::mail",
						`[skein] mail_unread seed failed for ${id}: ${String(err)}`,
					);
				});
		}
	}, [rooms]);

	// Returns whether mail is still pending for `harnessId` after this
	// run (`mailPending`, #386) — `false` only when there's definitely
	// nothing left to retry for (no meta at all); an invoke failure or a
	// missing activity record reads as still-pending `true`, so the
	// bounded retry keeps trying within its window rather than giving up
	// on what may be a transient gap.
	const check = async (harnessId: string): Promise<boolean> => {
		const meta = metaRef.current.get(harnessId);
		if (!meta) return false;
		// #388: settle a still-pending nudge BEFORE asking for the live
		// unread count — a rollback here has to land before
		// `decideMailNudge` sees this run's count, or it would compare
		// against the (wrong) post-nudge `lastNudged` instead of the
		// restored pre-nudge one.
		// #388: whether a nudge is still settling after the block below —
		// folded into the gate passed to `decideMailNudge` so a second
		// automatic nudge can't go out while the first one's submit is
		// still unproven. `seamSubmit` clears the composer draft
		// optimistically the moment `sendPrompt` writes the "\r", so the
		// draft alone can't hold this open the way it holds a manually
		// typed one — without this flag, mail arriving inside the
		// settlement window would paste a second copy on top of a first
		// nudge that may not have been submitted yet.
		//
		// Residual: if the rollback deadline passes with the first
		// nudge's Enter dropped through all of #380's own retries, the
		// text can still be sitting in the composer while the draft
		// reads clean — the re-paste this flag now allows would then
		// stack a second copy on top of it. Accepted: the field case
		// #388 exists for is a swallowed paste that never reached the
		// terminal at all (an empty composer), not a delivered paste
		// whose trailing Enter alone was lost after every retry.
		let settlementPending = false;
		const settlement = settleRef.current.get(harnessId);
		if (settlement) {
			const activity = harnessActivity.get(harnessId);
			const decision = evaluateSettlement({
				nowMs: Date.now(),
				settlement,
				activity:
					activity === null
						? null
						: {
								authoritative: activity.authoritative,
								adapterSilent: activity.adapterSilent,
								phase: activity.phase,
								delegationDeferredAt: activity.delegationDeferredAt,
							},
				inputCountNow: harnessInput.userInputCount(harnessId),
			});
			settlementPending = decision.settlementPending;
			if (decision.outcome === "confirmed") {
				clearSettlement(harnessId);
				logBoth(
					"info",
					"skein::mail",
					`[skein] useMailDelivery: nudge confirmed delivered for harness ${harnessId} (#388)`,
				);
			} else if (decision.outcome === "rollback") {
				// `restoreLastNudged` is always set on a rollback outcome —
				// `evaluateSettlement` never returns `null` alongside it.
				if (decision.restoreLastNudged !== null) {
					lastNudgedRef.current.set(harnessId, decision.restoreLastNudged);
				}
				clearSettlement(harnessId);
				logBoth(
					"info",
					"skein::mail",
					`[skein] useMailDelivery: rolling back nudge for harness ${harnessId} — no proof the submit landed (#388)`,
				);
				// #404: this same run's rollback may itself be why a #259
				// adapter-silent degrade fired — the nudge that armed the
				// watchdog never landed, so the tail was never given a real
				// prompt to answer. Recover it before the gate below runs,
				// but only when no human has driven the harness since the
				// nudge was pasted.
				if (decision.attemptRecovery) {
					if (harnessActivity.recoverUnheardSilence(harnessId)) {
						logBoth(
							"info",
							"skein::mail",
							`[skein] useMailDelivery: recovered harness ${harnessId} from a lost first paste before retrying (#404)`,
						);
					} else {
						logBoth(
							"info",
							"skein::mail",
							`[skein] useMailDelivery: declined to recover harness ${harnessId} from a lost first paste (#404)`,
						);
					}
				}
			}
			// "wait": leave the settlement armed and fall through to the
			// normal check below — its own timer, or the next event
			// trigger, will re-run this — but refuse a second automatic
			// nudge until it resolves (`settlementPending`, set above).
		}
		let res: MailUnread;
		try {
			res = await invoke<MailUnread>("mail_unread", {
				roomId: meta.roomId,
				harnessId,
			});
		} catch (err: unknown) {
			logBoth("warn", "skein::mail", `[skein] mail_unread failed for ${harnessId}: ${String(err)}`);
			return true;
		}
		mailStore.set(harnessId, res.count, res.fromRoomNames);
		const activity = harnessActivity.get(harnessId);
		if (!activity) return true;
		const body = mailNudgeText(res.count, res.fromRoomNames);
		// #388: a settlement still pending refuses outright, before even
		// consulting `canSendPrompt`/the draft — see the comment on
		// `settlementPending` above.
		const gate: GateResult = settlementPending
			? { ok: false, reason: "a previous nudge is still settling (#388)" }
			: automaticGate(
					canSendPrompt({
						capabilities: HARNESS_KINDS[meta.kind].capabilities,
						activity,
						registered: harnessInput.isRegistered(harnessId),
						bracketedPasteOn: harnessInput.bracketedPaste(harnessId),
						body,
					}),
					harnessInput.draft(harnessId),
				);
		const lastNudged = lastNudgedRef.current.get(harnessId) ?? 0;
		const decision = decideMailNudge({
			atStoppingPoint: atSafeStoppingPoint(activity),
			unread: res.count,
			lastNudged,
			gate,
		});
		if (!decision.nudge) {
			lastNudgedRef.current.set(harnessId, decision.lastNudged);
			// #404: only worth a log line when there's mail actually
			// outstanding AND the gate itself is why nothing went out —
			// "not at a stopping point yet" or "already nudged for this
			// count" aren't refusals, just not-yet.
			const pending = pendingResult(harnessId, res.count, decision.lastNudged);
			if (!gate.ok && pending) {
				logMailRefusal(harnessId, gate.reason);
			}
			return pending;
		}
		logBoth(
			"info",
			"skein::mail",
			`[skein] useMailDelivery: nudging harness ${harnessId} — unread=${res.count} lastNudged=${lastNudged} (#404)`,
		);
		// `sendPrompt` re-checks the gate at call time — a passing
		// `canSendPrompt` above can still lose a race to a phase flip
		// between the two. Only count it as nudged (even provisionally)
		// when the send actually went out; otherwise keep the pre-nudge
		// value (the reset a `nudge: false` decision would have produced)
		// so the next arrival or waiting transition tries again.
		//
		// #388: `result.ok` only means the gate passed and the body was
		// PASTED — the submit "\r" follows `SUBMIT_GAP_MS` later, and can
		// still be silently skipped (the user typed, the phase moved, the
		// terminal respawned; see harnessInput.ts's #380 gap-then-retry).
		// So a passing send doesn't finalize `lastNudged` here — it records
		// it PROVISIONALLY (so no second nudge goes out for this same
		// count while proof is pending) and arms a settlement
		// (`mailSettle.ts`) that either drops it once the submit is
		// proven to have landed, or rolls `lastNudged` back to its
		// pre-nudge value once `NUDGE_SETTLE_MS` passes with no proof —
		// see this settlement's own evaluation at the top of `check`.
		const result = sendPrompt(harnessId, meta.kind, body, { automatic: true });
		if (!result.ok) {
			lastNudgedRef.current.set(harnessId, lastNudged);
			logMailRefusal(harnessId, result.reason);
			return pendingResult(harnessId, res.count, lastNudged);
		}
		lastMailRefusalReasonRef.current.delete(harnessId);
		lastNudgedRef.current.set(harnessId, decision.lastNudged);
		const prior = settleRef.current.get(harnessId);
		if (prior) clearTimeout(prior.timer);
		const preNudge = prior !== undefined ? Math.min(prior.preNudge, lastNudged) : lastNudged;
		const timer = setTimeout(() => {
			runSerialized(harnessId, "event");
		}, NUDGE_SETTLE_MS);
		settleRef.current.set(harnessId, {
			preNudge,
			sentAtMs: Date.now(),
			deferredAtSend: activity.delegationDeferredAt,
			turnStarted: false,
			inputCountAtSend: harnessInput.userInputCount(harnessId),
			timer,
		});
		return pendingResult(harnessId, res.count, decision.lastNudged);
	};

	// #386: (re-)arm this harness's bounded retry after a `check` run —
	// `source: "event"` (any of the five triggers above) resets the
	// window to start now; `source: "timer"` (a retry tick calling
	// itself) keeps whatever window it was already on, arming one only
	// if somehow none was recorded. Only schedules while the harness is
	// still known at all — a harness this hook has already forgotten
	// (closed, room archived) never gets a new timer, even if its last
	// `check` result is still in flight when that happens.
	const scheduleRetry = (harnessId: string, source: "event" | "timer", pending: boolean): void => {
		const prior = retryRef.current.get(harnessId);
		if (prior?.timer) clearTimeout(prior.timer);
		if (!mountedRef.current || !metaRef.current.has(harnessId)) {
			clearRetry(harnessId);
			return;
		}
		const nowMs = Date.now();
		const armedAtMs = source === "event" ? nowMs : (prior?.armedAtMs ?? nowMs);
		const phase = harnessActivity.get(harnessId)?.phase ?? null;
		const decision = nextMailRetry({ pending, phase, armedAtMs, nowMs });
		if (decision.kind === "stop") {
			clearRetry(harnessId);
			return;
		}
		const timer = setTimeout(() => {
			runSerialized(harnessId, "timer");
		}, decision.delayMs);
		retryRef.current.set(harnessId, { armedAtMs, timer });
	};

	const runSerialized = (harnessId: string, source: "event" | "timer" = "event"): void => {
		const prior = inFlightRef.current.get(harnessId) ?? Promise.resolve(false);
		const next = prior.then(
			() => check(harnessId),
			() => check(harnessId),
		);
		inFlightRef.current.set(harnessId, next);
		void next.then((pending) => {
			scheduleRetry(harnessId, source, pending);
		});
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		const unlistenPromise = listen<{ roomId: string; harnessId: string }>(
			"skein://mail-changed",
			(event) => {
				const { harnessId } = event.payload;
				if (!metaRef.current.has(harnessId)) return;
				runSerialized(harnessId);
			},
		);
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);

	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized/clearRetry/clearSettlement close only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to) => {
			if (!metaRef.current.has(harnessId)) return;
			// #388: a transition leaving `waiting` into `running` or
			// `permission` is the same proof `submitRetry.ts` already uses
			// that a submit landed — mark it on any pending settlement so
			// the next `check` (this transition's own `runSerialized`
			// below, or the settlement's own timer) sees it.
			const settlement = settleRef.current.get(harnessId);
			if (settlement && from === "waiting" && (to === "running" || to === "permission")) {
				settlement.turnStarted = true;
			}
			// #386/#388: a harness that has exited has nothing left to
			// retry or settle for — stop its timer(s) right away rather
			// than waiting for the window to run out on its own.
			if (to === "exited") {
				clearRetry(harnessId);
				clearSettlement(harnessId);
			}
			const activity = harnessActivity.get(harnessId);
			const atStoppingPointNow = activity !== null && atSafeStoppingPoint(activity);
			if (!shouldCheckOnTransition(to, atStoppingPointNow)) return;
			runSerialized(harnessId);
		});
		return unsub;
	}, []);

	// #381: a #277 delegation deferral can arm with NO phase change at
	// all — the harness was already `running` and stays `running` — so
	// `subscribeTransitions` above never fires for it. This is the only
	// trigger for that case.
	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessActivity.subscribeDelegationDeferred((harnessId) => {
			if (!metaRef.current.has(harnessId)) return;
			runSerialized(harnessId);
		});
		return unsub;
	}, []);

	// #383: a composer draft clearing without a submit is the only
	// signal that held mail is now safe to retry — a submit's own
	// running → waiting transition is already covered above.
	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessInput.subscribeDraftCleared((harnessId) => {
			if (!metaRef.current.has(harnessId)) return;
			runSerialized(harnessId);
		});
		return unsub;
	}, []);

	// #386: a harness finishing registration in the #238 seam is the
	// trigger that closes this issue's original bug — mail that landed
	// while a fresh harness was still `spawning`, whose one
	// `spawning → waiting` check was missed or itself refused because
	// there was no seam yet to paste into.
	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessInput.subscribeRegistered((harnessId) => {
			if (!metaRef.current.has(harnessId)) return;
			runSerialized(harnessId);
		});
		return unsub;
	}, []);

	// #386/#388: stop every outstanding retry and settlement timer on
	// unmount — nothing left to schedule into once this hook is gone.
	useEffect(() => {
		const retries = retryRef.current;
		const settlements = settleRef.current;
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
			for (const entry of retries.values()) {
				if (entry.timer) clearTimeout(entry.timer);
			}
			retries.clear();
			for (const entry of settlements.values()) {
				clearTimeout(entry.timer);
			}
			settlements.clear();
		};
	}, []);
}
