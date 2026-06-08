// Flattened-item layer for the Activity feed — issue #80 D2d.
//
// The feed isn't a 1:1 map of actions to rows: some kinds become derived
// chrome (a turn_duration row becomes a turn separator) and some are
// consumed elsewhere entirely (turn_cost → cost hair-lines in a later
// slice, away_summary → the room subtitle, reasoning → not shown). This
// module turns the ordered action list into the list of things actually
// rendered, so the Activity card maps over *items*, and the auto-tail
// unseen-counter counts items rather than raw actions.
//
// Cost hair-lines (D2d-2) and the backfill banner/end-marker (D2d-3) add
// more item variants here; this slice ships rows + turn separators.

import { formatClock, formatDuration } from "./Row.tsx";
import { type Payload, num, parsePayload } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// A renderable entry in the feed: an action row, or a derived
/// turn-boundary separator.
export type FeedItem =
	| { type: "row"; key: string; action: HarnessAction }
	| {
			type: "separator";
			key: string;
			/** The turn_duration row's timestamp — the turn's *end*. */
			timestampMs: number;
			/** Turn length in ms (turn_duration.duration_ms), if recorded. */
			durationMs: number | undefined;
	  };

/// Kinds that never render as their own row and aren't separators either:
/// consumed elsewhere. Keep in sync with the dispatcher in rows.tsx
/// (whose default case returns null for exactly these).
const SKIP_KINDS = new Set(["turn_cost", "away_summary", "reasoning"]);

/// Flatten display-ordered actions into feed items: turn_duration → a
/// separator, SKIP_KINDS dropped, everything else → a row. Input must
/// already be in display order (see orderForDisplay).
export function flattenFeed(actions: HarnessAction[]): FeedItem[] {
	const items: FeedItem[] = [];
	for (const a of actions) {
		if (a.kind === "turn_duration") {
			const p: Payload = parsePayload(a.payload);
			items.push({
				type: "separator",
				key: `sep-${a.id}`,
				timestampMs: a.timestampMs,
				durationMs: num(p.duration_ms),
			});
		} else if (!SKIP_KINDS.has(a.kind)) {
			items.push({ type: "row", key: `row-${a.id}`, action: a });
		}
	}
	return items;
}

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
