// Presentational components for the Activity feed items. Split out of
// feedItems.tsx (#460).

import type { HarnessKind } from "../types.ts";
import { Row, formatClock, formatDuration } from "./Row.tsx";
import type { FeedItem } from "./feedItemTypes.ts";
import { formatTokensShort, formatUsd } from "./feedTotals.ts";

/// Head-styled session cost/token pair. Each half is omitted when it has
/// nothing to report, so a Claude room on a build with no `cost_state`
/// shows tokens alone rather than a misleading `$0.00`.
export const SessionTotals = ({ usd, tokens }: { usd: number; tokens: number }) => (
	<>
		{usd > 0 && <span>· {formatUsd(usd)}</span>}
		{tokens > 0 && <span>· {formatTokensShort(tokens)} tok</span>}
	</>
);

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
