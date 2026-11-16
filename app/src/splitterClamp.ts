// Pure sizing rule for Splitter's first pane.
//
// The divider is a flex sibling between the panes, so the second pane gets
// containerSize - first - divider; `divider` is its thickness (Splitter.css).

/**
 * Clamp the first pane's size to the container.
 *
 * - containerSize <= 0 means unmeasured (or inside a display:none parent):
 *   return `size` unchanged.
 * - The second pane itself (excluding the divider) keeps at least minSecond.
 * - When the container cannot fit both minimums, the second pane's minimum
 *   wins and the first pane shrinks below minFirst (down to 0): the divider
 *   and second pane must stay reachable, which is the bug this guards.
 */
export const clampSplit = (
	size: number,
	containerSize: number,
	minFirst: number,
	minSecond: number,
	divider = 0,
): number => {
	if (containerSize <= 0) return size;
	const upper = containerSize - minSecond - divider;
	if (upper < minFirst) return Math.max(0, upper);
	return Math.max(minFirst, Math.min(size, upper));
};
