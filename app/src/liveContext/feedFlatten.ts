// Flattened-item layer for the Activity feed — issue #80 D2d.
//
// The feed isn't a 1:1 map of actions to rows: some kinds become derived
// chrome (a turn_duration row becomes a turn separator, turn_cost rows
// become per-turn cost hair-lines), edit storms fold into burst items,
// and some kinds are consumed elsewhere entirely (away_summary → the
// room subtitle, reasoning → not shown). This module turns the ordered
// action list into the list of things actually rendered, so the Activity
// card maps over *items*, and the auto-tail unseen-counter counts items
// rather than raw actions.
//
// Provenance (D2d-3): backfilled rows are loaded once by the mount query
// and never broadcast, so "live" = the row arrived over the harness-action
// event (store.ts `liveIds`). The flattened feed marks row items with it
// (the slide-in animation keys off it) and splices a backfill banner
// before the first backfilled item plus an end-marker after the last one
// when live items follow.

import {
	BURST_GAP_MS,
	BURST_MIN,
	type BurstCandidate,
	burstCandidate,
	commonAncestor,
	costFromClaude,
	SKIP_KINDS,
	shortScope,
	stepFromOpencode,
} from "./feedItemsHelpers.ts";
import type { FeedItem, FlattenOptions } from "./feedItemTypes.ts";
import { num, type Payload, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// Flatten display-ordered actions into feed items: turn_duration → a
/// separator, turn_cost → a per-turn cost hair-line (when enabled),
/// SKIP_KINDS dropped, everything else → a row. Input must already be in
/// display order (see orderForDisplay).
///
/// Cost items land at their stream position: Claude's single terminal
/// turn_cost arrives just before its turn_duration, so the hair-line sits
/// directly above the separator (matching the prototype tape); opencode
/// has no separators, so the hair-line itself marks the turn boundary.
/// An opencode turn still in flight (no terminal step yet) emits nothing,
/// and one orphaned mid-flight (interrupted — next prompt arrives first)
/// is dropped rather than leaked into the following turn's line. Caveat:
/// live-streamed opencode steps are stamped ts=0 (#93), so their position
/// comes from the display-order carry and can drift from the turn's true
/// place in history.
///
/// Backfill chrome: the banner goes immediately before the first
/// backfilled-derived item and the end-marker after the last one (only
/// when live items follow — handover §11). Positional, not id-boundary,
/// because a live row can legitimately sort between backfilled rows
/// (multi-harness timestamp skew); such a row sits inside the marked
/// region rather than corrupting the markers.
///
/// Burst fold: runs of BURST_MIN+ qualifying patch rows (same harness /
/// tool / provenance, gaps < BURST_GAP_MS) collapse into one burst item
/// carrying its constituents, scoped to their deepest common directory.
/// A fold never crosses the backfill boundary — itemLive stays honest,
/// so the chrome splice does too. Streaks shorter than the minimum
/// re-emit as plain rows.
export function flattenFeed(actions: HarnessAction[], opts: FlattenOptions): FeedItem[] {
	const { showTurnCosts, liveIds } = opts;
	const items: FeedItem[] = [];
	// Per-item provenance, parallel to `items` — drives the splice below.
	const itemLive: boolean[] = [];
	const pushRaw = (item: FeedItem, live: boolean) => {
		items.push(item);
		itemLive.push(live);
	};
	// Pending same-tool same-dir patch streak (all entries share
	// harnessId, tool, dir, and provenance).
	let streak: BurstCandidate[] = [];
	const flushStreak = () => {
		if (streak.length === 0) return;
		const cands = streak;
		streak = [];
		const first = cands[0];
		const last = cands[cands.length - 1];
		if (!first || !last) return;
		if (cands.length < BURST_MIN) {
			for (const c of cands) {
				pushRaw({ type: "row", key: `row-${c.action.id}`, action: c.action, live: c.live }, c.live);
			}
			return;
		}
		// Totals only when every constituent reported a diff — a partial
		// sum rendered as "+12 −0" reads as definite. (Single rows omit
		// unknown deltas the same way.)
		const hasDeltas = cands.every((c) => c.adds != null || c.dels != null);
		// Single-dir storms keep the exact `…/src/auth/` scope; spanning
		// storms render their common ancestor as `skills/**` (recursive,
		// matching the prototype's burst scopes).
		const ancestor = commonAncestor(cands.map((c) => c.dir));
		const spans = cands.some((c) => c.dir.replace(/\\/g, "/") !== ancestor);
		pushRaw(
			{
				type: "burst",
				key: `burst-${first.action.id}`,
				harnessId: first.action.harnessId,
				tool: first.tool,
				dir: ancestor,
				scope: spans ? (ancestor ? `${shortScope(ancestor)}**` : "**") : shortScope(ancestor),
				adds: hasDeltas ? cands.reduce((n, c) => n + (c.adds ?? 0), 0) : undefined,
				dels: hasDeltas ? cands.reduce((n, c) => n + (c.dels ?? 0), 0) : undefined,
				firstTimestampMs: first.action.timestampMs,
				lastTimestampMs: last.action.timestampMs,
				live: first.live,
				actions: cands.map((c) => c.action),
			},
			first.live,
		);
	};
	// Any non-streak emission visibly interrupts a storm, so it closes
	// the pending fold first.
	const push = (item: FeedItem, live: boolean) => {
		flushStreak();
		pushRaw(item, live);
	};
	// Claude re-emits a turn's cost row verbatim sometimes; request_id
	// identifies the API call, so it de-dupes them.
	const seenRequestIds = new Set<string>();
	// opencode re-emissions have no id at all — de-dupe by payload identity
	// (see the shape comment above).
	const seenSteps = new Set<string>();
	// Per-harness running totals for opencode's per-step cost rows.
	const openTurns = new Map<string, { tokens: number; usd: number }>();
	for (const a of actions) {
		const live = liveIds.has(a.id);
		if (a.kind === "patch") {
			const cand = burstCandidate(a, live);
			if (cand) {
				const prev = streak[streak.length - 1];
				if (
					prev &&
					prev.action.harnessId === a.harnessId &&
					prev.tool === cand.tool &&
					prev.live === cand.live &&
					a.timestampMs - prev.action.timestampMs < BURST_GAP_MS
				) {
					streak.push(cand);
				} else {
					flushStreak();
					streak = [cand];
				}
				continue;
			}
			// Non-candidate patch (errored, snapshot flavor, no file):
			// renders as its own row and breaks any pending streak.
			push({ type: "row", key: `row-${a.id}`, action: a, live }, live);
		} else if (a.kind === "turn_duration") {
			const p: Payload = parsePayload(a.payload);
			push(
				{
					type: "separator",
					key: `sep-${a.id}`,
					timestampMs: a.timestampMs,
					durationMs: num(p.duration_ms),
				},
				live,
			);
		} else if (a.kind === "turn_cost") {
			if (!showTurnCosts) continue;
			const p: Payload = parsePayload(a.payload);
			if ("usage" in p || "stop_reason" in p) {
				const cost = costFromClaude(p);
				if (!cost || cost.tokens <= 0) continue;
				// Register the request_id only for an emission that counts,
				// so a degenerate row can't swallow a later real one.
				const requestId = str(p.request_id);
				if (requestId) {
					if (seenRequestIds.has(requestId)) continue;
					seenRequestIds.add(requestId);
				}
				push({ type: "cost", key: `cost-${a.id}`, ...cost }, live);
			} else if ("tokens" in p || "reason" in p) {
				const stepKey = `${a.harnessId}|${a.payload}`;
				if (seenSteps.has(stepKey)) continue;
				seenSteps.add(stepKey);
				const acc = openTurns.get(a.harnessId) ?? { tokens: 0, usd: 0 };
				const step = stepFromOpencode(p);
				acc.tokens += step.tokens;
				acc.usd += step.usd;
				if (!step.terminal) {
					openTurns.set(a.harnessId, acc);
				} else {
					openTurns.delete(a.harnessId);
					if (acc.tokens > 0 || acc.usd > 0) {
						// A turn spanning the boundary (backfilled steps,
						// live terminal) counts live — it completed on watch.
						push({ type: "cost", key: `cost-${a.id}`, ...acc }, live);
					}
				}
			}
		} else {
			// A prompt starts this harness's next turn: steps still
			// accumulated here belong to a turn that never reached a
			// terminal step — drop them, don't bill them forward.
			if (a.kind === "user_prompt") openTurns.delete(a.harnessId);
			// Null-title ai_title rows render nothing (rows.tsx) — emitting
			// an invisible item would break streaks and count as unseen
			// activity the user can't see.
			if (a.kind === "ai_title" && !str(parsePayload(a.payload).ai_title)) continue;
			if (!SKIP_KINDS.has(a.kind)) {
				push({ type: "row", key: `row-${a.id}`, action: a, live }, live);
			}
		}
	}
	// A streak at the very tail is still a burst (it may keep growing —
	// the card decides live shimmer at render time).
	flushStreak();

	// Splice in the backfill chrome. The end-marker first — it sits at the
	// higher index, so the banner's insertion below can't shift it.
	const firstBackfilled = itemLive.indexOf(false);
	if (firstBackfilled !== -1) {
		const lastBackfilled = itemLive.lastIndexOf(false);
		// Everything past the last backfilled item is live by construction,
		// so "live items follow" is exactly "the last item isn't backfilled".
		// (A live row sorted above it — timestamp skew — must not summon a
		// marker that would dangle at the bottom claiming "live below".)
		if (lastBackfilled !== itemLive.length - 1) {
			items.splice(lastBackfilled + 1, 0, { type: "backfill-end", key: "backfill-end" });
		}
		let count = 0;
		let rangeStartMs = 0;
		let rangeEndMs = 0;
		for (const a of actions) {
			if (liveIds.has(a.id)) continue;
			count++;
			if (a.timestampMs > 0) {
				if (rangeStartMs === 0 || a.timestampMs < rangeStartMs) rangeStartMs = a.timestampMs;
				if (a.timestampMs > rangeEndMs) rangeEndMs = a.timestampMs;
			}
		}
		items.splice(firstBackfilled, 0, {
			type: "backfill-banner",
			key: "backfill-banner",
			count,
			rangeStartMs,
			rangeEndMs,
		});
	}
	return items;
}
