// Pure text of the room subtitle (#764), split from RoomSubtitle.tsx so it
// is testable without a DOM.

export interface Subtitle {
	glyph: string;
	text: string;
	age: string;
	empty: boolean;
}

export function relativeAge(ms: number, now: number): string {
	if (ms <= 0) return "";
	const secs = Math.max(0, Math.round((now - ms) / 1000));
	if (secs < 60) return `${secs}s ago`;
	const mins = Math.round(secs / 60);
	if (mins < 60) return `${mins}m ago`;
	const hrs = Math.round(mins / 60);
	if (hrs < 24) return `${hrs}h ago`;
	return `${Math.round(hrs / 24)}d ago`;
}

/// A Claude recap wins verbatim. Without one, a room whose feed has rows
/// gets a neutral line (opencode, Hermes and shell rooms never have a
/// recap); only a room with nothing in the feed reads as idle.
export function subtitleFor(
	awaySummary: { content: string; timestampMs: number } | null,
	activityCount: number,
	lastActionTs: number,
	now: number,
): Subtitle {
	if (awaySummary) {
		return {
			glyph: "AT",
			text: awaySummary.content,
			age: relativeAge(awaySummary.timestampMs, now),
			empty: false,
		};
	}
	if (activityCount > 0) {
		return {
			glyph: "AT",
			text: "No away recap yet",
			age: relativeAge(lastActionTs, now),
			empty: true,
		};
	}
	return { glyph: "IDLE", text: "No agent has worked here yet", age: "", empty: true };
}
