// Wording for an element thread shown in its JSX source file (#467).

import type { ReviewThread } from "./api.ts";

const baseName = (p: string): string => p.split(/[/]/).pop() ?? p;

export function sourceNoteText(thread: ReviewThread, inline: boolean): string {
	const home = baseName(thread.filePath ?? "the design pane");
	if (thread.lineStart != null && !thread.outdated) {
		return inline
			? `Design comment on ${home} — shown here at its source line`
			: `Design comment on ${home} — at line ${thread.lineStart}, which this diff doesn't show`;
	}
	const src = thread.element?.anchor.source;
	const at = src ? `${src.file}:${src.line}` : "its source line";
	return src?.lineText != null || thread.anchorLines.length > 0
		? `Recorded at ${at}; that line can't be confirmed in this file any more`
		: `Recorded at ${at}, from the design pane; open the design pane to re-check`;
}
