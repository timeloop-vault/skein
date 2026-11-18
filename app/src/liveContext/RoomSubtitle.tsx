// Room subtitle — the always-visible bar between the tab strip and the
// card stack. Hosts the latest Claude `away_summary` verbatim; without
// one, a neutral "no recap" line when the feed has activity, and the idle
// empty state only when it has none (#764). Read-only in v1 (handover §4).

import { subtitleFor } from "./subtitle.ts";

export const RoomSubtitle = ({
	awaySummary,
	activityCount,
	lastActionTs,
}: {
	awaySummary: { content: string; timestampMs: number } | null;
	activityCount: number;
	lastActionTs: number;
}) => {
	const subtitle = subtitleFor(awaySummary, activityCount, lastActionTs, Date.now());
	return (
		<div className={`lc-subtitle ${subtitle.empty ? "is-empty" : ""}`}>
			<span className="glyph">{subtitle.glyph}</span>
			<span className="text">{subtitle.text}</span>
			{subtitle.age !== "" && <span className="meta">{subtitle.age}</span>}
		</div>
	);
};
