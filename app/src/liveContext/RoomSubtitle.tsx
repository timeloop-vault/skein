// Room subtitle — the always-visible bar between the tab strip and the
// card stack. Hosts the latest Claude `away_summary` verbatim, or an
// idle empty state. Read-only in v1 (handover §4).

import type { HarnessAction } from "./store.ts";

interface Subtitle {
	text: string;
	age: string;
	empty: boolean;
}

export const RoomSubtitle = ({ actions }: { actions: HarnessAction[] }) => {
	const subtitle = deriveSubtitle(actions);
	return (
		<div className={`lc-subtitle ${subtitle.empty ? "is-empty" : ""}`}>
			<span className="glyph">{subtitle.empty ? "IDLE" : "AT"}</span>
			<span className="text">{subtitle.text}</span>
			{!subtitle.empty && <span className="meta">{subtitle.age}</span>}
		</div>
	);
};

/// Latest `away_summary` action becomes the subtitle; otherwise the
/// empty state. away_summary payload is `{ content: string }`.
function deriveSubtitle(actions: HarnessAction[]): Subtitle {
	for (let i = actions.length - 1; i >= 0; i--) {
		const a = actions[i];
		if (a && a.kind === "away_summary") {
			let content = "";
			try {
				content = (JSON.parse(a.payload) as { content?: string }).content ?? "";
			} catch {
				content = "";
			}
			if (content) {
				return { text: content, age: relativeAge(a.timestampMs), empty: false };
			}
		}
	}
	return { text: "No agent has worked here yet", age: "", empty: true };
}

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
