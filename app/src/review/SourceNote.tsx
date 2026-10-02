// The note an element thread wears when it is shown in its JSX source file
// rather than its home entry HTML (#467). Same thread either way — this
// only says where it really lives and how sure Skein is of the line.

import type { ReviewThread } from "./api.ts";
import { sourceNoteText } from "./sourceNote.ts";
import "./element.css";

export const SourceNote = ({ thread, inline }: { thread: ReviewThread; inline: boolean }) => {
	if (!thread.viaSource) return null;
	const confirmed = thread.lineStart != null && !thread.outdated;
	return (
		<div className={`rv-sourcenote${confirmed ? "" : " rv-outdated"}`}>
			<span>{sourceNoteText(thread, inline)}</span>
			{!confirmed && thread.anchorLines.length > 0 && (
				<pre className="rv-anchor">{thread.anchorLines.join("\n")}</pre>
			)}
		</div>
	);
};
