// Pure feed-order and unseen-counter logic for the activity card.

import type { FeedItem } from "./feedItems.tsx";
import type { HarnessAction } from "./store.ts";

/// The last-seen-bottom marker: the bottom content item's key, plus its
/// constituent count when it was a burst (growth past it is unseen).
export interface BottomMark {
	key: string;
	burstSize: number | undefined;
}

/// The action id a content-item key encodes (`row-<id>` / `burst-<first
/// id>`), for re-resolving a marker whose item folded or dissolved.
function keyActionId(key: string): number | undefined {
	const m = /^(?:row|burst)-(\d+)$/.exec(key);
	return m?.[1] ? Number(m[1]) : undefined;
}

/// Re-order id-ordered rows chronologically. Each row's effective
/// timestamp is its own when > 0, else carried forward from the prior
/// row; the carry is seeded with the first real timestamp so a run of
/// `ts=0` rows at the head doesn't sort above everything. Stable on
/// (effectiveTs, id).
export function orderForDisplay(actions: HarnessAction[]): HarnessAction[] {
	let carry = 0;
	for (const a of actions) {
		if (a.timestampMs > 0) {
			carry = a.timestampMs;
			break;
		}
	}
	const withEff = actions.map((a) => {
		const eff = a.timestampMs > 0 ? a.timestampMs : carry;
		carry = eff;
		return { a, eff };
	});
	withEff.sort((x, y) => x.eff - y.eff || x.a.id - y.a.id);
	return withEff.map((w) => w.a);
}

/// The marker for the last *content* item — a row or a burst, never
/// chrome — so it always names real activity even when a separator is
/// the tail item. For a burst it also records the constituent count.
export function bottomMarkOf(items: FeedItem[]): BottomMark | undefined {
	for (let i = items.length - 1; i >= 0; i--) {
		const it = items[i];
		if (it?.type === "row") return { key: it.key, burstSize: undefined };
		if (it?.type === "burst") return { key: it.key, burstSize: it.actions.length };
	}
	return undefined;
}

/// Unseen activity = the row/burst items after the last-seen bottom
/// (chrome isn't counted), plus the marker burst's own growth.
export function countUnseen(
	items: FeedItem[],
	atBottom: boolean,
	seenBottom: BottomMark | undefined,
): number {
	if (atBottom) return 0;
	let start = 0;
	let n = 0;
	if (seenBottom !== undefined) {
		let idx = items.findIndex((it) => it.key === seenBottom.key);
		if (idx === -1) {
			// The marker item may have folded into a burst (row → burst)
			// or dissolved back into rows (burst → rows) since it was
			// recorded — resolve by the action id both key shapes encode.
			// Fold direction under-counts the burst's later constituents;
			// better slight under-count than the whole feed.
			const id = keyActionId(seenBottom.key);
			if (id !== undefined) {
				idx = items.findIndex(
					(it) =>
						(it.type === "row" && it.action.id === id) ||
						(it.type === "burst" && it.actions.some((a) => a.id === id)),
				);
			}
		}
		// Marker gone entirely: count nothing rather than everything.
		if (idx === -1) return 0;
		const at = items[idx];
		if (at?.type === "burst" && at.key === seenBottom.key && seenBottom.burstSize != null) {
			n += Math.max(0, at.actions.length - seenBottom.burstSize);
		}
		start = idx + 1;
	}
	for (let i = start; i < items.length; i++) {
		const it = items[i];
		if (it?.type === "row") n++;
		else if (it?.type === "burst") n += it.actions.length;
	}
	return n;
}
