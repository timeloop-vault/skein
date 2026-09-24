// Text-shaping for the generic hover-tooltip trigger (`data-sk-tip`,
// statusPopover.ts #355) — a caller hands the popover a full nudge/action
// body, which can be a page-long #358 override, and this caps it to a
// sensible number of lines so the popover never grows to screen size.
// Pure so it's covered without a DOM (tipText.test.ts).

/** How many lines of a `data-sk-tip` body the popover renders before
 *  truncating — long enough to show a real multi-line nudge in full,
 *  short enough that an unbounded override body can't fill the screen. */
export const TIP_MAX_LINES = 12;

/** Caps `text` to `maxLines` lines, appending an ellipsis line when it
 *  had to cut. Newlines are preserved (the caller renders them verbatim,
 *  e.g. `white-space: pre-wrap`) — only the line COUNT is capped, not
 *  the width of any one line. */
export function truncateTip(text: string, maxLines: number = TIP_MAX_LINES): string {
	const lines = text.split("\n");
	if (lines.length <= maxLines) return text;
	return `${lines.slice(0, maxLines).join("\n")}\n…`;
}
