// Flattened-item layer for the Activity feed — issue #80 D2d.
//
// The feed isn't a 1:1 map of actions to rows: some kinds become derived
// chrome (a turn_duration row becomes a turn separator, turn_cost rows
// become per-turn cost hair-lines) and some are consumed elsewhere
// entirely (away_summary → the room subtitle, reasoning → not shown).
// This module turns the ordered action list into the list of things
// actually rendered, so the Activity card maps over *items*, and the
// auto-tail unseen-counter counts items rather than raw actions.
//
// The backfill banner/end-marker (D2d-3) adds more item variants here.

import { formatClock, formatDuration } from "./Row.tsx";
import { type Payload, num, obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// A renderable entry in the feed: an action row, a derived turn-boundary
/// separator, or a per-turn cost hair-line.
export type FeedItem =
	| { type: "row"; key: string; action: HarnessAction }
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
	  };

/// Kinds that never render as their own row and aren't derived chrome
/// either: consumed elsewhere. Keep in sync with the dispatcher in
/// rows.tsx, whose default case returns null for these (plus turn_cost /
/// turn_duration, which this module turns into derived items instead).
const SKIP_KINDS = new Set(["away_summary", "reasoning"]);

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
///   step's reason.
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

/// Flatten display-ordered actions into feed items: turn_duration → a
/// separator, turn_cost → a per-turn cost hair-line (when enabled),
/// SKIP_KINDS dropped, everything else → a row. Input must already be in
/// display order (see orderForDisplay).
///
/// Cost items land at their stream position: Claude's single terminal
/// turn_cost arrives just before its turn_duration, so the hair-line sits
/// directly above the separator (matching the prototype tape); opencode
/// has no separators, so the hair-line itself marks the turn boundary.
/// An opencode turn still in flight (no terminal step yet) emits nothing.
export function flattenFeed(actions: HarnessAction[], showTurnCosts: boolean): FeedItem[] {
	const items: FeedItem[] = [];
	// Claude re-emits a turn's cost row verbatim sometimes; request_id
	// identifies the API call, so it de-dupes them.
	const seenRequestIds = new Set<string>();
	// Per-harness running totals for opencode's per-step cost rows.
	const openTurns = new Map<string, { tokens: number; usd: number }>();
	for (const a of actions) {
		if (a.kind === "turn_duration") {
			const p: Payload = parsePayload(a.payload);
			items.push({
				type: "separator",
				key: `sep-${a.id}`,
				timestampMs: a.timestampMs,
				durationMs: num(p.duration_ms),
			});
		} else if (a.kind === "turn_cost") {
			if (!showTurnCosts) continue;
			const p: Payload = parsePayload(a.payload);
			if ("usage" in p || "stop_reason" in p) {
				const requestId = str(p.request_id);
				if (requestId) {
					if (seenRequestIds.has(requestId)) continue;
					seenRequestIds.add(requestId);
				}
				const cost = costFromClaude(p);
				if (cost && cost.tokens > 0) {
					items.push({ type: "cost", key: `cost-${a.id}`, ...cost });
				}
			} else if ("tokens" in p || "reason" in p) {
				const acc = openTurns.get(a.harnessId) ?? { tokens: 0, usd: 0 };
				const step = stepFromOpencode(p);
				acc.tokens += step.tokens;
				acc.usd += step.usd;
				if (!step.terminal) {
					openTurns.set(a.harnessId, acc);
				} else {
					openTurns.delete(a.harnessId);
					if (acc.tokens > 0 || acc.usd > 0) {
						items.push({ type: "cost", key: `cost-${a.id}`, ...acc });
					}
				}
			}
		} else if (!SKIP_KINDS.has(a.kind)) {
			items.push({ type: "row", key: `row-${a.id}`, action: a });
		}
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
