// Room subtitle — the always-visible bar between the tab strip and the
// card stack. Hosts the latest Claude `away_summary` verbatim, or an
// idle empty state. Read-only in v1 (handover §4).

interface Subtitle {
	text: string;
	age: string;
	empty: boolean;
}

export const RoomSubtitle = ({
	awaySummary,
}: {
	awaySummary: { content: string; timestampMs: number } | null;
}) => {
	const subtitle: Subtitle = awaySummary
		? { text: awaySummary.content, age: relativeAge(awaySummary.timestampMs), empty: false }
		: { text: "No agent has worked here yet", age: "", empty: true };
	return (
		<div className={`lc-subtitle ${subtitle.empty ? "is-empty" : ""}`}>
			<span className="glyph">{subtitle.empty ? "IDLE" : "AT"}</span>
			<span className="text">{subtitle.text}</span>
			{!subtitle.empty && <span className="meta">{subtitle.age}</span>}
		</div>
	);
};

function relativeAge(ms: number): string {
	if (ms <= 0) return "";
	const secs = Math.max(0, Math.round((Date.now() - ms) / 1000));
	if (secs < 60) return `${secs}s ago`;
	const mins = Math.round(secs / 60);
	if (mins < 60) return `${mins}m ago`;
	const hrs = Math.round(mins / 60);
	if (hrs < 24) return `${hrs}h ago`;
	return `${Math.round(hrs / 24)}d ago`;
}
