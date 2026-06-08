// Activity card body. Renders actions chronologically with auto-tail.
//
// Ordering: the store keeps rows in insertion (id) order, but that
// isn't chronological — backfill inserts each harness's history as a
// separate batch, and older rows predate the current extractor. So we
// re-order for display by timestamp, carrying a timestamp forward (and
// seeding from the first real one) for `ts=0` rows so they stay next to
// their insertion neighbours instead of flying to the top. Backend
// ts-stamping + dedup is tracked in #93.
//
// Auto-tail: the feed sticks to the bottom while you're at the bottom;
// scroll up and a "▼ N new" pill appears, click to jump back to live.
// The unseen count is *derived* from a last-seen-bottom marker (not a
// running delta) so an older backfilled row inserted mid-list doesn't
// inflate it. The `visible` prop matters because every room's card stays
// mounted (display:none toggled) — a hidden card can't measure scroll,
// so we re-pin when it becomes visible. Turn separators / cost lines /
// backfill banner / burst are D2d–D2e; virtualization D2g (the
// per-event re-sort is acceptable until then).

import { useEffect, useMemo, useRef, useState } from "react";
import type { HarnessKind } from "../types.ts";
import "./activity.css";
import { TurnSeparator, flattenFeed } from "./feedItems.tsx";
import { ActivityRow } from "./rows.tsx";
import type { HarnessAction } from "./store.ts";

/// Pixels from the bottom within which the feed counts as "at the
/// bottom" and keeps auto-tailing.
const TAIL_THRESHOLD_PX = 30;

/// Re-order id-ordered rows chronologically. Each row's effective
/// timestamp is its own when > 0, else carried forward from the prior
/// row; the carry is seeded with the first real timestamp so a run of
/// `ts=0` rows at the head doesn't sort above everything. Stable on
/// (effectiveTs, id).
function orderForDisplay(actions: HarnessAction[]): HarnessAction[] {
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

export const ActivityCardBody = ({
	actions,
	harnessKindOf,
	visible,
}: {
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	visible: boolean;
}) => {
	const ordered = useMemo(() => orderForDisplay(actions), [actions]);
	// Flatten to rendered items (rows + derived turn separators). The feed
	// maps over these, and the unseen-counter counts them — not raw
	// actions — so derived/dropped kinds don't skew the "N new" pill.
	const items = useMemo(() => flattenFeed(ordered), [ordered]);
	// The marker tracks the last *row* (separators are chrome), so it
	// always names a real activity entry even when a turn separator is the
	// tail item. Behaviourally identical to the last-item key for counting
	// — rows after either index match — but keeps the marker contract true.
	const bottomKey = useMemo(() => {
		for (let i = items.length - 1; i >= 0; i--) {
			const it = items[i];
			if (it?.type === "row") return it.key;
		}
		return undefined;
	}, [items]);

	const scrollerRef = useRef<HTMLDivElement>(null);
	const [atBottom, setAtBottom] = useState(true);
	// Key of the bottom item the last time the user was at the bottom.
	const [seenBottomKey, setSeenBottomKey] = useState<string | undefined>(undefined);

	// Unseen rows = the *row* items after the last-seen bottom (separators
	// aren't counted — they're chrome, not new activity). Derived, so a
	// mid-list insert of an older row (which sorts above the marker)
	// doesn't count, and the pill reflects what's actually below.
	const newCount = useMemo(() => {
		if (atBottom) return 0;
		const start =
			seenBottomKey === undefined ? 0 : items.findIndex((it) => it.key === seenBottomKey) + 1;
		let n = 0;
		for (let i = Math.max(start, 0); i < items.length; i++) {
			if (items[i]?.type === "row") n++;
		}
		return n;
	}, [items, atBottom, seenBottomKey]);

	// Pin to the bottom while tailing. Skipped while hidden (a
	// display:none card can't measure); re-runs and catches up the moment
	// `visible` flips true.
	useEffect(() => {
		const el = scrollerRef.current;
		if (!el || !visible) return;
		// Re-pin on any content growth, not just a tail-key change, so a
		// mid-list insert while tailing still keeps us at the bottom.
		if (atBottom && items.length > 0) {
			el.scrollTop = el.scrollHeight;
			setSeenBottomKey(bottomKey);
		}
	}, [items, atBottom, visible, bottomKey]);

	const onScroll = () => {
		const el = scrollerRef.current;
		if (!el) return;
		const bottom = el.scrollHeight - el.scrollTop - el.clientHeight < TAIL_THRESHOLD_PX;
		setAtBottom(bottom);
		if (bottom) setSeenBottomKey(bottomKey);
	};

	const jumpToLatest = () => {
		const el = scrollerRef.current;
		if (el) el.scrollTop = el.scrollHeight;
		setAtBottom(true);
		setSeenBottomKey(bottomKey);
	};

	if (items.length === 0) {
		return (
			<div className="lc-empty">
				<div className="lc-empty-inner">
					<div className="big">☰</div>
					activity will appear here as agents work
				</div>
			</div>
		);
	}

	return (
		<div className="lc-activity">
			<div className="lc-activity-scroll" ref={scrollerRef} onScroll={onScroll}>
				{items.map((it) =>
					it.type === "separator" ? (
						<TurnSeparator key={it.key} timestampMs={it.timestampMs} durationMs={it.durationMs} />
					) : (
						<ActivityRow key={it.key} row={it.action} harnessKindOf={harnessKindOf} />
					),
				)}
				<div className="lc-tail">
					<span className="blinker" />
					tailing — new rows appear live
				</div>
			</div>
			{!atBottom && newCount > 0 && (
				<button type="button" className="lc-newbelow" onClick={jumpToLatest}>
					▼ {newCount} new
				</button>
			)}
		</div>
	);
};
