// #329: delivers agent mail into a waiting harness by pasting a nudge
// through the #238 seam (`harnessInput.sendPrompt`), and keeps
// `mailStore.ts`'s per-harness unread marker fresh. Called once from
// App.tsx with the current (non-archived) rooms — it owns no UI of its
// own; the tab marker lives in `components.tsx`'s `HarnessTab` via
// `useUnreadMail`.
//
// Four triggers run the same `check(harnessId)`:
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
//     above.
//
// All three go through `runSerialized` per harness so triggers landing
// together can't double-nudge — `decideMailNudge` itself is pure and
// stateless per call, so the ordering has to be enforced here.
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
	// concurrently.
	const inFlightRef = useRef<Map<string, Promise<void>>>(new Map());

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

	const check = async (harnessId: string): Promise<void> => {
		const meta = metaRef.current.get(harnessId);
		if (!meta) return;
		let res: MailUnread;
		try {
			res = await invoke<MailUnread>("mail_unread", {
				roomId: meta.roomId,
				harnessId,
			});
		} catch (err: unknown) {
			console.warn(`[skein] mail_unread failed for ${harnessId}:`, err);
			return;
		}
		mailStore.set(harnessId, res.count, res.fromRoomNames);
		const activity = harnessActivity.get(harnessId);
		if (!activity) return;
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
			return;
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
		lastNudgedRef.current.set(harnessId, result.ok ? decision.lastNudged : lastNudged);
	};

	const runSerialized = (harnessId: string): void => {
		const prior = inFlightRef.current.get(harnessId) ?? Promise.resolve();
		const next = prior.then(
			() => check(harnessId),
			() => check(harnessId),
		);
		inFlightRef.current.set(harnessId, next);
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

	// biome-ignore lint/correctness/useExhaustiveDependencies: runSerialized closes only over refs, stable across renders.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, _from, to) => {
			if (!metaRef.current.has(harnessId)) return;
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
}
