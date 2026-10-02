// The note an element thread wears when it is shown in its JSX source file
// rather than its home entry HTML (#467). Same thread either way — this
// only says where it really lives and how sure Skein is of the line. The
// page's own state (anchored/unknown) is deliberately absent: the mirror's
// placement depends only on its line text.

import type { ReviewThread } from "./api.ts";
import { sourceHeaderText, sourceNoteText } from "./sourceNote.ts";
import type { DesignLink } from "./Thread.tsx";
import "./element.css";

const NO_DESIGN_TITLE = "Add a design harness to this room to see it";

export const SourceNote = ({
	thread,
	inline,
	design,
}: {
	thread: ReviewThread;
	inline: boolean;
	design?: DesignLink | undefined;
}) => {
	if (!thread.viaSource) return null;
	const confirmed = thread.lineStart != null && !thread.outdated;
	const text = sourceNoteText(thread, inline);
	return (
		<div className={`rv-sourcenote${confirmed ? "" : " rv-outdated"}`}>
			<div className="rv-sourcenote-head">
				<span title={thread.filePath ?? undefined}>{sourceHeaderText(thread)}</span>
				{design && (
					<button
						type="button"
						className="rv-linkbtn"
						disabled={!design.available}
						title={design.available ? "focus this element in the design pane" : NO_DESIGN_TITLE}
						onClick={design.onShow}
					>
						show in design pane
					</button>
				)}
			</div>
			{text !== null && <span>{text}</span>}
			{!confirmed && thread.anchorLines.length > 0 && (
				<pre className="rv-anchor">{thread.anchorLines.join("\n")}</pre>
			)}
		</div>
	);
};
