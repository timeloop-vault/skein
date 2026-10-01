import { useEffect, useMemo, useRef, useState } from "react";
import type { HarnessAction } from "./store.ts";

/// Silence after which the tail sentinel reads "idle" with a static dot
/// (handover §5.3 item 6; the 90 s figure is §12's suggestion, tunable).
/// Shared with the card head's "· idle 2h 14m" meta in LiveContext.
export const IDLE_AFTER_MS = 90_000;

/// The room's last-activity instant, for the idle displays: the newest
/// action timestamp, OR the receipt time of the newest live arrival —
/// whichever is later. The receipt half matters because live rows can
/// carry ts=0 (opencode step rows): a tool-less opencode answer emits
/// only those, and on timestamps alone the room would read "idle" while
/// the answer streams in.
export function useIdleBasis(actions: HarnessAction[], liveIds: ReadonlySet<number>): number {
	const lastActionTs = useMemo(() => {
		let m = 0;
		for (const a of actions) if (a.timestampMs > m) m = a.timestampMs;
		return m;
	}, [actions]);
	const liveCountRef = useRef(0);
	const [lastLiveArrival, setLastLiveArrival] = useState(0);
	useEffect(() => {
		if (actions.length === 0) return;
		// liveIds mutates in lockstep with `actions` updates, so this
		// effect observes every growth on the render it causes.
		if (liveIds.size > liveCountRef.current) {
			liveCountRef.current = liveIds.size;
			setLastLiveArrival(Date.now());
		}
	}, [actions, liveIds]);
	return Math.max(lastActionTs, lastLiveArrival);
}
