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
import { logBoth } from "./frontendLog.ts";
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import { harnessInput } from "./harnessInput.ts";
import { checkMail } from "./mailDeliveryCheck.ts";
import {
	type HarnessMeta,
	type MailRefs,
	type MailUnread,
	type RetryEntry,
	type SettleEntry,
	clearRetry,
	clearSettlement,
} from "./mailDeliveryState.ts";
import { mailHold } from "./mailHold.ts";
import { shouldCheckOnTransition } from "./mailNudge.ts";
import { nextMailRetry } from "./mailRetry.ts";
import { mailStore } from "./mailStore.ts";
import { forgetMailState, noteMailState } from "./supervisor/mailFeed.ts";
import type { Room } from "./types.ts";

export function useMailDelivery(rooms: readonly Room[]): void {
	// All per-harness state lives in refs (documented on `MailRefs`) so the
	// event/transition listeners, mounted once with empty deps, always see
	// the latest values without re-subscribing.
	const metaRef = useRef<Map<string, HarnessMeta>>(new Map());
	const seededRef = useRef<Set<string>>(new Set());
	const lastNudgedRef = useRef<Map<string, number>>(new Map());
	const inFlightRef = useRef<Map<string, Promise<boolean>>>(new Map());
	// #386: per-harness bounded-retry state.
	const retryRef = useRef<Map<string, RetryEntry>>(new Map());
	// #388: per-harness settlement state for a nudge not yet proven submitted.
	const settleRef = useRef<Map<string, SettleEntry>>(new Map());
	const mountedRef = useRef(true);
	const lastMailRefusalReasonRef = useRef<Map<string, string>>(new Map());
	const refs: MailRefs = {
		metaRef,
		seededRef,
		lastNudgedRef,
		inFlightRef,
		retryRef,
		settleRef,
		mountedRef,
		lastMailRefusalReasonRef,
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
			forgetMailState(id);
			mailStore.forget(id);
			mailHold.forget(id);
			clearRetry(refs, id);
			clearSettlement(refs, id);
		}
		for (const id of seen) {
			if (seededRef.current.has(id)) continue;
			seededRef.current.add(id);
			const m = meta.get(id);
			if (!m) continue;
			invoke<MailUnread>("mail_unread", { roomId: m.roomId, harnessId: id })
				.then((res) => {
					mailStore.set(id, res.count, res.fromRoomNames);
					noteMailState(id, { unread: res.count });
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

	const check = (harnessId: string, releasedByUser = false): Promise<boolean> =>
		checkMail(refs, (id) => runSerialized(id, "event"), harnessId, releasedByUser);

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
			clearRetry(refs, harnessId);
			return;
		}
		const nowMs = Date.now();
		const armedAtMs = source === "event" ? nowMs : (prior?.armedAtMs ?? nowMs);
		const phase = harnessActivity.get(harnessId)?.phase ?? null;
		const decision = nextMailRetry({ pending, phase, armedAtMs, nowMs });
		if (decision.kind === "stop") {
			clearRetry(refs, harnessId);
			return;
		}
		const timer = setTimeout(() => {
			runSerialized(harnessId, "timer");
		}, decision.delayMs);
		retryRef.current.set(harnessId, { armedAtMs, timer });
	};

	const runSerialized = (
		harnessId: string,
		source: "event" | "timer" = "event",
		releasedByUser = false,
	): void => {
		const prior = inFlightRef.current.get(harnessId) ?? Promise.resolve(false);
		const next = prior.then(
			() => check(harnessId, releasedByUser),
			() => check(harnessId, releasedByUser),
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
				clearRetry(refs, harnessId);
				clearSettlement(refs, harnessId);
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
			// #413: a fresh PTY starts with a clean composer; a "held" marker
			// from the old one would flash until the next check.
			mailHold.forget(harnessId);
			if (!metaRef.current.has(harnessId)) return;
			runSerialized(harnessId);
		});
		return unsub;
	}, []);

	// #413: the tab's "deliver now" click.
	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		return mailHold.onRelease((harnessId) => {
			if (!metaRef.current.has(harnessId)) return;
			runSerialized(harnessId, "event", true);
		});
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
