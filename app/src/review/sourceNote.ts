// Wording for an element thread shown in its JSX source file (#467).

import type { ReviewThread } from "./api.ts";

const baseName = (p: string): string => p.split(/[/]/).pop() ?? p;

const clip = (s: string, max: number): string => (s.length > max ? `${s.slice(0, max)}…` : s);

/// The one compact header line: `◆ design · <entry> · <tag> <id or selector>`.
/// Page content, so clipped; the "show in design pane" button follows it.
export function sourceHeaderText(thread: ReviewThread): string {
	const parts = ["◆ design", baseName(thread.filePath ?? "design pane")];
	const a = thread.element?.anchor;
	if (a) parts.push(`${clip(a.tag, 24)} ${clip(a.odId ?? a.selector, 48)}`);
	return parts.join(" · ");
}

/// What the mirror adds beyond the header, or null when it is placed inline
/// and has nothing more to say. Placement here depends only on the line
/// text, so nothing about the page's own state belongs in it.
export function sourceNoteText(thread: ReviewThread, inline: boolean): string | null {
	if (thread.lineStart != null && !thread.outdated) {
		return inline ? null : `at line ${thread.lineStart}, which this diff doesn't show`;
	}
	const src = thread.element?.anchor.source;
	const at = src ? `${src.file}:${src.line}` : "its source line";
	return src?.lineText != null || thread.anchorLines.length > 0
		? `Recorded at ${at}; that line can't be confirmed in this file any more`
		: `Recorded at ${at}, from the design pane; open the design pane to re-check`;
}
