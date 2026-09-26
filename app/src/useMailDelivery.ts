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
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "./harnessInput.ts";
import {
	automaticGate,
	decideMailNudge,
	mailNudgeText,
	shouldCheckOnTransition,
} from "./mailNudge.ts";
import { mailPending, nextMailRetry } from "./mailRetry.ts";
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
	// False once the hook has unmounted, so a `check` still in flight at
	// that moment can't schedule a timer the unmount cleanup already missed.
	const mountedRef = useRef(true);

	const clearRetry = (harnessId: string): void => {
		const entry = retryRef.current.get(harnessId);
		if (entry?.timer) clearTimeout(entry.timer);
		retryRef.current.delete(harnessId);
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: clearRetry closes only over refs, stable across renders.
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
			mailStore.forget(id);
			clearRetry(id);
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
					console.warn(`[skein] mail_unread seed failed for ${id}:`, err);
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
		let res: MailUnread;
		try {
			res = await invoke<MailUnread>("mail_unread", {
				roomId: meta.roomId,
				harnessId,
			});
		} catch (err: unknown) {
			console.warn(`[skein] mail_unread failed for ${harnessId}:`, err);
			return true;
		}
		mailStore.set(harnessId, res.count, res.fromRoomNames);
		const activity = harnessActivity.get(harnessId);
		if (!activity) return true;
		const body = mailNudgeText(res.count, res.fromRoomNames);
		const gate = automaticGate(
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
			return mailPending(res.count, decision.lastNudged);
		}
		// `sendPrompt` re-checks the gate at call time — a passing
		// `canSendPrompt` above can still lose a race to a phase flip
		// between the two. Only count it as nudged when the send
		// actually went out; otherwise keep the pre-nudge value (the
		// reset a `nudge: false` decision would have produced) so the
		// next arrival or waiting transition tries again.
		//
		// #380: `result.ok` only means the gate passed and the body was
		// pasted — the submit "\r" follows `SUBMIT_GAP_MS` later, and
		// can still be skipped (the user typed, the phase moved, the
		// terminal respawned) without `sendPrompt` reporting back here.
		// A skipped submit still counts as nudged, deliberately: the
		// text is now visible, sitting in the harness's own composer,
		// and re-nudging on the next tick would paste a second copy on
		// top of it rather than fix anything.
		const result = sendPrompt(harnessId, meta.kind, body, { automatic: true });
		const finalLastNudged = result.ok ? decision.lastNudged : lastNudged;
		lastNudgedRef.current.set(harnessId, finalLastNudged);
		return mailPending(res.count, finalLastNudged);
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

	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized/clearRetry close only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, _from, to) => {
			if (!metaRef.current.has(harnessId)) return;
			// #386: a harness that has exited has nothing left to retry
			// for — stop its timer right away rather than waiting for the
			// window to run out on its own.
			if (to === "exited") clearRetry(harnessId);
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

	// #386: stop every outstanding retry timer on unmount — nothing left
	// to schedule into once this hook is gone.
	useEffect(() => {
		const retries = retryRef.current;
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
			for (const entry of retries.values()) {
				if (entry.timer) clearTimeout(entry.timer);
			}
			retries.clear();
		};
	}, []);
}
