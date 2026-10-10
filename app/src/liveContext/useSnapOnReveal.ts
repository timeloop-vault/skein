// Rows that arrived while the card was hidden must snap in on reveal, not
// slide in all at once (#593). The store parks hidden rows and flushes them
// in a layout effect on reveal, so `items` changes in the same transition as
// `visible` — too late for an effect keyed on `!visible` to have seen them.
// Instead, the render that flips `visible` true arms a one-shot window and
// every live row rendered inside it is marked already-animated. The window
// closes on a macrotask: the store's flush re-render lands synchronously
// before paint, well ahead of it.

import { useEffect, useRef } from "react";
import type { FeedItem } from "./feedItems.tsx";

export function useSnapOnReveal(
	items: readonly FeedItem[],
	visible: boolean,
	animatedKeys: { current: Set<string> },
): void {
	const prevVisible = useRef(visible);
	const armed = useRef(false);
	if (visible && !prevVisible.current) armed.current = true;
	prevVisible.current = visible;
	if (armed.current) {
		for (const it of items) {
			if (it.type === "row" && it.live) animatedKeys.current.add(it.key);
		}
	}
	useEffect(() => {
		if (!armed.current) return;
		const t = setTimeout(() => {
			armed.current = false;
		}, 0);
		return () => clearTimeout(t);
	});
}
