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
// The feed is virtualized via react-virtuoso (D2g): only ~viewport rows
// are in the DOM, so a heavy room no longer force-lays-out thousands of
// rows when it reveals. Virtuoso owns the scroll + stick-to-bottom
// (followOutput); we keep the unseen-counter / idle / slide-in / burst /
// backfill machinery, all of which work off the in-memory `items` array,
// not the DOM.
//
// Auto-tail: followOutput keeps us pinned while at the bottom; scroll up
// and a "▼ N new" pill appears, click to jump back to live. The unseen
// count is *derived* from a last-seen-bottom marker (not a running
// delta) so an older backfilled row inserted mid-list doesn't inflate
// it. The `visible` prop matters because every room's card stays mounted
// (display:none toggled) — Virtuoso can't measure while hidden, so we
// re-assert the bottom when it becomes visible.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import type { HarnessKind } from "../types.ts";
import "./activity.css";
import {
	BackfillBanner,
	BackfillEnd,
	BurstRow,
	type FeedItem,
	TurnCost,
	TurnSeparator,
	flattenFeed,
} from "./feedItems.tsx";
import { ActivityRow } from "./rows.tsx";
import type { HarnessAction } from "./store.ts";

/// Pixels from the bottom within which the feed counts as "at the
/// bottom" and keeps auto-tailing.
const TAIL_THRESHOLD_PX = 30;

/// How long after its newest constituent a burst keeps the live shimmer.
/// Matches the fold gap — while another edit could still join, the storm
/// is plausibly ongoing.
const BURST_LIVE_MS = 5000;

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

/// The last-seen-bottom marker: the bottom content item's key, plus its
/// constituent count when it was a burst (growth past it is unseen).
interface BottomMark {
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

export const ActivityCardBody = ({
	actions,
	liveIds,
	harnessKindOf,
	visible,
	showTurnCosts,
}: {
	actions: HarnessAction[];
	/** Ids that arrived over the live event channel (store.ts). Stable
	 *  identity, mutated in lockstep with `actions` — additions never
	 *  re-render on their own, only via the `actions` update they ride. */
	liveIds: ReadonlySet<number>;
	harnessKindOf: (harnessId: string) => HarnessKind;
	visible: boolean;
	/** Render per-turn cost hair-lines (user-level pref, off by default). */
	showTurnCosts: boolean;
}) => {
	const ordered = useMemo(() => orderForDisplay(actions), [actions]);
	// Flatten to rendered items (rows + derived turn separators / cost
	// lines + backfill chrome). The feed maps over these, and the
	// unseen-counter counts them — not raw actions — so derived/dropped
	// kinds don't skew the "N new" pill. (Chrome items are never counted.)
	const items = useMemo(
		() => flattenFeed(ordered, { showTurnCosts, liveIds }),
		[ordered, showTurnCosts, liveIds],
	);

	// Keys whose slide-in already played (or never should). A one-shot CSS
	// animation restarts from 0% whenever its element re-enters layout, and
	// every room's card sits under display:none while inactive — without
	// this, switching rooms would replay every live row's slide at once.
	// onAnimationEnd marks rows that animated to completion; the effect
	// below marks everything whenever the card is hidden, which covers rows
	// that arrive while hidden (they snap on reveal) and animations cut
	// short by a room switch.
	const animatedKeys = useRef<Set<string>>(new Set());
	useEffect(() => {
		if (!visible) {
			for (const it of items) {
				if (it.type === "row" && it.live) animatedKeys.current.add(it.key);
			}
		}
	}, [items, visible]);
	// Expanded bursts, by burst key (stable: the first constituent's id,
	// so growth doesn't collapse an open burst).
	const [expandedBursts, setExpandedBursts] = useState<ReadonlySet<string>>(new Set());
	const toggleBurst = (key: string) =>
		setExpandedBursts((prev) => {
			const next = new Set(prev);
			if (next.has(key)) {
				next.delete(key);
			} else {
				next.add(key);
			}
			return next;
		});

	// Expanded tool-result previews, by preview key (`pv-<action id>`).
	// Held here, not in ResultPreview, so an expanded preview survives the
	// row unmounting when the virtualized feed scrolls it off (D2g).
	const [expandedPreviews, setExpandedPreviews] = useState<ReadonlySet<string>>(new Set());
	const togglePreview = useCallback((key: string) => {
		setExpandedPreviews((prev) => {
			const next = new Set(prev);
			if (next.has(key)) {
				next.delete(key);
			} else {
				next.add(key);
			}
			return next;
		});
	}, []);

	// Re-render at the EARLIEST upcoming shimmer lapse (one burst at a
	// time — a single max-based timer would let an older live burst
	// overstay by up to the full window); ticking re-runs the effect to
	// arm the next lapse. Liveness itself is computed at render time from
	// the burst's last constituent timestamp.
	const [liveTick, setLiveTick] = useState(0);
	// biome-ignore lint/correctness/useExhaustiveDependencies: liveTick is the re-arm trigger — each tick re-runs the effect to schedule the next future lapse, though the body never reads it.
	useEffect(() => {
		const now = Date.now();
		let next = Number.POSITIVE_INFINITY;
		for (const it of items) {
			if (it.type === "burst" && it.live) {
				const lapse = it.lastTimestampMs + BURST_LIVE_MS;
				if (lapse > now) next = Math.min(next, lapse);
			}
		}
		if (!Number.isFinite(next)) return;
		const t = setTimeout(() => setLiveTick((n) => n + 1), next - now + 50);
		return () => clearTimeout(t);
	}, [items, liveTick]);

	// Tail sentinel idle state: the room's been silent past the
	// threshold. Computed at render time from the idle basis; the
	// timeout re-renders once when the threshold passes.
	const idleBasis = useIdleBasis(actions, liveIds);
	const [idleTick, setIdleTick] = useState(0);
	// biome-ignore lint/correctness/useExhaustiveDependencies: idleTick is the re-arm trigger — it re-runs the effect after the flip so a future-stamped row can re-schedule, though the body never reads it.
	useEffect(() => {
		if (idleBasis <= 0) return;
		const delta = idleBasis + IDLE_AFTER_MS - Date.now();
		if (delta <= 0) return;
		const t = setTimeout(() => setIdleTick((n) => n + 1), delta + 50);
		return () => clearTimeout(t);
	}, [idleBasis, idleTick]);
	const tailIdle = idleBasis > 0 && Date.now() - idleBasis > IDLE_AFTER_MS;

	// Constituents of an on-screen live burst have been shown (folded);
	// pre-mark their row keys so a later un-fold (a retro-sorted row
	// splitting the streak below the minimum) re-renders them as plain
	// rows without replaying the slide-in.
	useEffect(() => {
		for (const it of items) {
			if (it.type === "burst" && it.live) {
				for (const a of it.actions) animatedKeys.current.add(`row-${a.id}`);
			}
		}
	}, [items]);

	// The marker tracks the last *content* item — a row or a burst, never
	// chrome — so it always names real activity even when a separator is
	// the tail item. For a burst it also records the constituent count at
	// the time, so the storm growing *inside* the marker item still reads
	// as unseen activity.
	const bottomMark = useMemo<BottomMark | undefined>(() => {
		for (let i = items.length - 1; i >= 0; i--) {
			const it = items[i];
			if (it?.type === "row") return { key: it.key, burstSize: undefined };
			if (it?.type === "burst") return { key: it.key, burstSize: it.actions.length };
		}
		return undefined;
	}, [items]);

	const virtuosoRef = useRef<VirtuosoHandle>(null);
	const [atBottom, setAtBottom] = useState(true);
	// The bottom item the last time the user was at the bottom.
	const [seenBottom, setSeenBottom] = useState<BottomMark | undefined>(undefined);
	const markSeenBottom = useCallback(() => {
		// Bail on equal content so per-event renders while pinned don't
		// churn state with fresh-but-identical marker objects.
		setSeenBottom((prev) =>
			prev && bottomMark && prev.key === bottomMark.key && prev.burstSize === bottomMark.burstSize
				? prev
				: bottomMark,
		);
	}, [bottomMark]);

	// Unseen activity = the row/burst items after the last-seen bottom
	// (chrome isn't counted), plus the marker burst's own growth. Bursts
	// weigh their constituent count — a 12-edit storm is 12 events,
	// matching the card head's units (the buildmap's "bursts collapse
	// N→1" parenthetical reads as 1-per-burst; deliberate deviation).
	// Derived, so a mid-list insert of an older row (which sorts above
	// the marker) doesn't count, and the pill reflects what's below.
	const newCount = useMemo(() => {
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
	}, [items, atBottom, seenBottom]);

	// Keep the seen-bottom marker pinned to the live tail while the user
	// is at the bottom (and the card visible), so the "N new" pill stays
	// at 0; once they scroll up we stop marking and it counts. Virtuoso's
	// followOutput does the actual scroll-to-bottom — we no longer read
	// scrollHeight: only ~viewport rows are in the DOM now, so there's
	// nothing to force-lay-out when a room reveals (the D2g point).
	// markSeenBottom's identity already changes whenever the bottom item
	// does (it closes over bottomMark, derived from items), so listing it
	// is enough to re-fire as the tail grows.
	useEffect(() => {
		if (atBottom && visible) markSeenBottom();
	}, [atBottom, visible, markSeenBottom]);

	// A room reveals (display:none → flex) without remounting; Virtuoso
	// couldn't measure while hidden, so re-assert the bottom on the flip
	// if we were tailing. Only on the visibility transition.
	// biome-ignore lint/correctness/useExhaustiveDependencies: re-pin only when `visible` flips; atBottom/items are read as guards, not triggers.
	useEffect(() => {
		if (visible && atBottom && items.length > 0) {
			virtuosoRef.current?.scrollToIndex({ index: items.length - 1, align: "end" });
		}
	}, [visible]);

	const jumpToLatest = () => {
		virtuosoRef.current?.scrollToIndex({ index: items.length - 1, align: "end" });
		setAtBottom(true);
		markSeenBottom();
	};

	// The tail sentinel becomes Virtuoso's Footer (it renders after the
	// last item). Memoised so it only re-creates when the idle state
	// flips, not on every scroll tick.
	const tailComponents = useMemo(
		() => ({
			Footer: () => (
				<div className={`lc-tail${tailIdle ? " idle" : ""}`}>
					<span className="blinker" />
					{tailIdle ? "idle" : "tailing — new rows appear live"}
				</div>
			),
		}),
		[tailIdle],
	);

	// One feed item → its element. Virtuoso owns the key (computeItemKey)
	// and the positioning, so nothing here carries a key except the
	// expanded-burst constituents (a list within one item).
	const renderItem = (item: FeedItem) => {
		switch (item.type) {
			case "separator":
				return <TurnSeparator timestampMs={item.timestampMs} durationMs={item.durationMs} />;
			case "cost":
				return <TurnCost tokens={item.tokens} usd={item.usd} />;
			case "backfill-banner":
				return (
					<BackfillBanner
						count={item.count}
						rangeStartMs={item.rangeStartMs}
						rangeEndMs={item.rangeEndMs}
					/>
				);
			case "backfill-end":
				return <BackfillEnd />;
			case "burst":
				return expandedBursts.has(item.key) ? (
					<>
						<div className="lc-burst-head">
							<span className="label">
								burst expanded · {item.actions.length} {item.tool}s in {item.scope}
							</span>
							<button type="button" className="collapse" onClick={() => toggleBurst(item.key)}>
								collapse
							</button>
						</div>
						{item.actions.map((a) => (
							<ActivityRow
								key={`row-${a.id}`}
								row={a}
								harnessKindOf={harnessKindOf}
								expandedPreviews={expandedPreviews}
								onTogglePreview={togglePreview}
							/>
						))}
					</>
				) : (
					<BurstRow
						item={item}
						harness={harnessKindOf(item.harnessId)}
						live={item.live && Date.now() - item.lastTimestampMs < BURST_LIVE_MS}
						onToggle={() => toggleBurst(item.key)}
					/>
				);
			case "row":
				return (
					<div
						className={
							item.live && !animatedKeys.current.has(item.key) ? "row-slide-in" : undefined
						}
						onAnimationEnd={(e) => {
							if (e.animationName === "lc-slide-in") animatedKeys.current.add(item.key);
						}}
					>
						<ActivityRow
							row={item.action}
							harnessKindOf={harnessKindOf}
							expandedPreviews={expandedPreviews}
							onTogglePreview={togglePreview}
						/>
					</div>
				);
		}
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
			<Virtuoso
				ref={virtuosoRef}
				className="lc-activity-scroll"
				style={{ height: "100%" }}
				data={items}
				computeItemKey={(_index, item) => item.key}
				itemContent={(_index, item) => renderItem(item)}
				// followOutput="auto": stick to the bottom only while already
				// there — Virtuoso owns the scroll, replacing the old
				// scrollHeight pin. atBottomStateChange drives our pill.
				followOutput="auto"
				atBottomThreshold={TAIL_THRESHOLD_PX}
				atBottomStateChange={setAtBottom}
				initialTopMostItemIndex={Math.max(0, items.length - 1)}
				increaseViewportBy={400}
				components={tailComponents}
			/>
			{!atBottom && newCount > 0 && (
				<button type="button" className="lc-newbelow" onClick={jumpToLatest}>
					▼ {newCount} new
				</button>
			)}
		</div>
	);
};
