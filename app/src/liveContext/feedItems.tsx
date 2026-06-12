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

import type { HarnessKind } from "../types.ts";
import { Row, formatClock, formatDuration } from "./Row.tsx";
import { type Payload, num, obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// A renderable entry in the feed: an action row, a derived turn-boundary
/// separator, a per-turn cost hair-line, or backfill-boundary chrome.
export type FeedItem =
	| {
			type: "row";
			key: string;
			action: HarnessAction;
			/** Arrived over the live event channel (vs the backfill query).
			 *  Live rows slide in; backfilled rows snap. */
			live: boolean;
	  }
	| {
			type: "separator";
			key: string;
			/** The turn_duration row's timestamp — the turn's *end*. */
			timestampMs: number;
			/** Turn length in ms (turn_duration.duration_ms), if recorded. */
			durationMs: number | undefined;
	  }
	| {
			type: "cost";
			key: string;
			/** Tokens the turn processed: input + output + cache reads/writes. */
			tokens: number;
			/** Turn cost in USD. 0 when the harness doesn't report it (Claude
			 *  has no cost field — backend gap #91). */
			usd: number;
	  }
	| {
			type: "backfill-banner";
			key: string;
			/** Backfilled action count (raw events, same unit as the card
			 *  head's "N events"). */
			count: number;
			/** First/last real timestamp among backfilled actions; 0 when
			 *  none carries a clock. */
			rangeStartMs: number;
			rangeEndMs: number;
	  }
	| { type: "backfill-end"; key: string }
	| {
			type: "burst";
			/** `burst-<first constituent id>` — stable as the burst grows. */
			key: string;
			harnessId: string;
			/** Normalized tool name (edit / write / multiedit). */
			tool: string;
			/** The constituents' shared directory (fold key), full path. */
			dir: string;
			/** Display scope — `dir` shortened for the gist. */
			scope: string;
			/** Cumulative additions/deletions; undefined when no constituent
			 *  carried patch_info (e.g. fresh Writes with no diff). */
			adds: number | undefined;
			dels: number | undefined;
			/** First/last constituent timestamps — the burst window. */
			firstTimestampMs: number;
			lastTimestampMs: number;
			/** Constituent provenance (uniform — a fold never crosses the
			 *  backfill boundary, or the chrome splice would lie). */
			live: boolean;
			/** The folded rows, in order, for the expanded view. */
			actions: HarnessAction[];
	  };

/// Kinds that never render as their own row and aren't derived chrome
/// either: consumed elsewhere. Keep in sync with the dispatcher in
/// rows.tsx, whose default case returns null for these (plus turn_cost /
/// turn_duration, which this module turns into derived items instead).
const SKIP_KINDS = new Set(["away_summary", "reasoning"]);

/// Burst fold rule (handover §6, the §12-confirmed variant of the spec's
/// three mutually inconsistent statements): consecutive emitted items
/// from the same harness, same normalized tool, same dirname, gap < 5 s.
/// "Consecutive" is over *items* — kinds that emit nothing (reasoning,
/// suppressed turn_cost) don't break a streak the user can't see broken.
const BURST_GAP_MS = 5000;
/// Minimum constituents before folding: a pair isn't a storm, and folding
/// it would hide detail for no real de-noising. Tune per §12.
const BURST_MIN = 3;

interface BurstCandidate {
	action: HarnessAction;
	dir: string;
	tool: string;
	adds: number | undefined;
	dels: number | undefined;
	live: boolean;
}

/// Path without its last segment ("" for bare filenames). Handles both
/// separators — harnesses emit native paths, and a "/"-only split would
/// collapse every Windows fold key to "", merging unrelated directories
/// into one repo-wide burst.
function dirname(path: string): string {
	const trimmed = path.replace(/[\\/]+$/, "");
	const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
	return i === -1 ? "" : trimmed.slice(0, i);
}

/// Shorten an (often absolute) directory for the burst gist: the last
/// two segments with a trailing slash, "…/"-prefixed when deeper.
function shortScope(dir: string): string {
	if (!dir) return "./";
	const segs = dir.split(/[\\/]/).filter(Boolean);
	const tail = segs.slice(-2).join("/");
	return `${segs.length > 2 ? "…/" : ""}${tail}/`;
}

/// A patch row that may fold into a burst: a successful edit/write with
/// a file and a real timestamp. Excluded on purpose: errored patches
/// (they render as error rows and should visibly break a storm) and the
/// opencode `{files, hash}` snapshot flavor (no tool, no deltas, ts=0).
function burstCandidate(a: HarnessAction, live: boolean): BurstCandidate | undefined {
	if (a.timestampMs <= 0) return undefined;
	const p: Payload = parsePayload(a.payload);
	if (p.is_error === true) return undefined;
	const raw = (str(p.tool) ?? "").toLowerCase();
	if (raw !== "edit" && raw !== "write" && raw !== "multiedit") return undefined;
	const files = Array.isArray(p.files) ? p.files : [];
	const file = str(files[0]);
	if (!file) return undefined;
	const pi = obj(p.patch_info);
	return {
		action: a,
		dir: dirname(file),
		// multiedit folds (and labels) as "write" — EditRow renders it as
		// write, and a fold key the user can't see would split visually
		// identical rows.
		tool: raw === "multiedit" ? "write" : raw,
		adds: pi ? num(pi.additions) : undefined,
		dels: pi ? num(pi.deletions) : undefined,
		live,
	};
}

/// The two turn_cost payload shapes (see docs/live-context-d2-buildmap.md
/// "Claude ↔ opencode payload divergence"):
///
/// - Claude — `{model, request_id, stop_reason, usage}`, one row per
///   terminal assistant turn; `usage` is the whole turn's totals already.
///   Real-data anomalies: duplicate emissions sharing a `request_id`, and
///   degenerate `model: "<synthetic>"` zero-usage rows — both dropped.
/// - opencode — `{cost, reason, snapshot, tokens}`, one row per model
///   *step* (many per turn); `reason: "tool-calls"` marks an intermediate
///   step, anything else ends the turn. Values are per-step deltas, so a
///   turn's cost is the sum over its steps. There is no per-turn grouping
///   key in the payload (`source` is null, `snapshot` is a worktree hash),
///   so steps are accumulated per harness and flushed on the terminal
///   step's reason. Backfill can re-insert a live-written step a second
///   time (#93); token totals grow monotonically through a session, so a
///   byte-identical (harness, payload) repeat is a re-emission, never a
///   real step — dropped, or the sums double.
///
/// Shapes are disjoint, so detection is by payload keys, not harness kind.
function costFromClaude(p: Payload): { tokens: number; usd: number } | undefined {
	if (str(p.model) === "<synthetic>") return undefined;
	const usage = obj(p.usage);
	if (!usage) return undefined;
	const tokens =
		(num(usage.input_tokens) ?? 0) +
		(num(usage.output_tokens) ?? 0) +
		(num(usage.cache_creation_input_tokens) ?? 0) +
		(num(usage.cache_read_input_tokens) ?? 0);
	return { tokens, usd: 0 };
}

function stepFromOpencode(p: Payload): { tokens: number; usd: number; terminal: boolean } {
	const t = obj(p.tokens);
	const tokens =
		num(t?.total) ??
		(num(t?.input) ?? 0) +
			(num(t?.output) ?? 0) +
			(num(t?.reasoning) ?? 0) +
			(num(obj(t?.cache)?.read) ?? 0) +
			(num(obj(t?.cache)?.write) ?? 0);
	return { tokens, usd: num(p.cost) ?? 0, terminal: str(p.reason) !== "tool-calls" };
}

export interface FlattenOptions {
	/** Render per-turn cost hair-lines (user pref, off by default). */
	showTurnCosts: boolean;
	/** Action ids that arrived over the live event channel (store.ts). */
	liveIds: ReadonlySet<number>;
}

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
/// tool / dirname / provenance, gaps < BURST_GAP_MS) collapse into one
/// burst item carrying its constituents. A fold never crosses the
/// backfill boundary — itemLive stays honest, so the chrome splice does
/// too. Streaks shorter than the minimum re-emit as plain rows.
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
		pushRaw(
			{
				type: "burst",
				key: `burst-${first.action.id}`,
				harnessId: first.action.harnessId,
				tool: first.tool,
				dir: first.dir,
				scope: shortScope(first.dir),
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
					prev.dir === cand.dir &&
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

/// Sub-cent costs render as a floor rather than a misleading "$0.00".
function formatUsd(usd: number): string {
	return usd < 0.005 ? "<$0.01" : `$${usd.toFixed(2)}`;
}

/// Per-turn cost hair-line — `4,218 tok  $0.18` under the turn's rows
/// (`.lc-turn-cost`, shipped with the D2a CSS block). Tokens are exact
/// and comma-grouped per the prototype; the k-abbreviated style is
/// reserved for the card head's session totals. Segments the harness
/// didn't report are omitted, not zero-filled.
export const TurnCost = ({ tokens, usd }: { tokens: number; usd: number }) => (
	<div className="lc-turn-cost">
		{tokens > 0 && (
			<span>
				<span className="v">{tokens.toLocaleString("en-US")}</span> tok
			</span>
		)}
		{usd > 0 && (
			<span>
				<span className="v">{formatUsd(usd)}</span>
			</span>
		)}
	</div>
);

/// Epoch-ms → "HH:MM" for the banner's window range — the handover drops
/// seconds there (row clocks keep them). Empty for missing clocks.
function formatClockShort(ms: number): string {
	if (!ms || ms <= 0) return "";
	const d = new Date(ms);
	const p = (n: number) => String(n).padStart(2, "0");
	return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

/// `↩ backfilled from disk · N events · 09:14 – 13:51` — precedes the
/// first backfilled item (handover §11). The range collapses to a single
/// clock when start and end share a minute, and is omitted entirely when
/// no backfilled row carries a timestamp.
export const BackfillBanner = ({
	count,
	rangeStartMs,
	rangeEndMs,
}: {
	count: number;
	rangeStartMs: number;
	rangeEndMs: number;
}) => {
	const start = formatClockShort(rangeStartMs);
	const end = formatClockShort(rangeEndMs);
	const range = start && end ? (start === end ? start : `${start} – ${end}`) : "";
	return (
		<div className="lc-backfill-banner">
			<span className="glyph">↩</span>
			<span className="text">
				backfilled from disk · <b>{count}</b> events
				{range && (
					<>
						{" · "}
						<span className="dim">{range}</span>
					</>
				)}
			</span>
		</div>
	);
};

/// Collapsed burst — `edit ×12 …/skein-core/src/ · click to expand` with
/// cumulative deltas and the storm window on the right. `live` (decided
/// by the card at render time: provenance-live and still inside the fold
/// window) drives the shimmer class and the accent pill.
export const BurstRow = ({
	item,
	harness,
	live,
	onToggle,
}: {
	item: Extract<FeedItem, { type: "burst" }>;
	harness: HarnessKind;
	live: boolean;
	onToggle: () => void;
}) => {
	const span = item.lastTimestampMs - item.firstTimestampMs;
	return (
		<Row
			kind="burst"
			className={live ? "live" : undefined}
			harness={harness}
			timestampMs={item.firstTimestampMs}
			onClick={onToggle}
			right={
				<>
					{item.adds != null && <span className="delta-add">+{item.adds}</span>}{" "}
					{item.dels != null && <span className="delta-del">−{item.dels}</span>}{" "}
					{span > 0 && <span className="dim">in {formatDuration(span)}</span>}
				</>
			}
		>
			<span className="tool">
				{item.tool} ×{item.actions.length}
			</span>{" "}
			<span className="target" title={item.dir || undefined}>
				{item.scope}
			</span>
			<span className="dim"> · click to expand</span>
			{live && <span className="pill live">live</span>}
		</Row>
	);
};

/// `─ resume tailing — live below ─` — follows the last backfilled item,
/// only when live items actually follow (handover §11; the prototype
/// renders it unconditionally, the handover wins).
export const BackfillEnd = () => (
	<div className="lc-backfill-end">
		<span className="line" />
		<span className="text">resume tailing — live below</span>
		<span className="line" />
	</div>
);

/// Turn boundary — a hair-line stamped `turn · <start>` on the left and
/// `<end> · <duration>` on the right. The turn_duration row marks the
/// end, so the start is derived as end − duration. Clocks are omitted
/// when the row carries no real timestamp (stamped 0).
export const TurnSeparator = ({
	timestampMs,
	durationMs,
}: {
	timestampMs: number;
	durationMs: number | undefined;
}) => {
	const endClock = formatClock(timestampMs);
	const startClock =
		durationMs != null && timestampMs > 0 ? formatClock(timestampMs - durationMs) : "";
	const label = startClock ? `turn · ${startClock}` : "turn";
	const right = [endClock, durationMs != null ? formatDuration(durationMs) : ""]
		.filter(Boolean)
		.join(" · ");
	return (
		<div className="lc-turn-sep">
			<span className="stamp">{label}</span>
			{right && <span className="right-stamp">{right}</span>}
		</div>
	);
};
