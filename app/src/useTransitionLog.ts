// L6 — append every real phase transition to the sqlite event log
// (#459 split of useHarnessNotifications; no behaviour change).
// Per-transition fire-and-forget; errors warn but don't surface UX. The
// log feeds (eventually) the L7 cross-harness activity feed; in the
// meantime the data exists for any "since last visit" surface to build
// on. Epic #50 L6.

import { invoke } from "@tauri-apps/api/core";
import type { MutableRefObject } from "react";
import { useEffect } from "react";
import { harnessActivity } from "./harnessActivity.ts";
import type { Room } from "./types.ts";

export function useTransitionLog(roomsRef: MutableRefObject<Room[]>) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			const owningRoom = roomsRef.current.find((r) => r.harnesses.some((h) => h.id === harnessId));
			if (!owningRoom) {
				// Transition for a harness that's no longer in
				// state — e.g. exit firing after the room was
				// archived. Without a roomId we can't usefully log;
				// skip.
				return;
			}
			const activity = harnessActivity.get(harnessId);
			void invoke("db_record_harness_event", {
				harnessId,
				roomId: owningRoom.id,
				fromPhase: from,
				toPhase: to,
				timestampMs: Date.now(),
				hasUserInput: activity?.hasUserInput ?? false,
				// L7a (#73): per-transition attribution.
				// Identifies which strategy fired it (`l2a-idle`,
				// `l2b-pattern`, `l2c1-claude-end-turn`, etc.) for
				// the eventual L7c activity feed.
				source,
			}).catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] db_record_harness_event failed for ${harnessId}:`, msg);
			});
		});
		return unsub;
	}, []);
}
