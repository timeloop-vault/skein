// Focus return for transient overlays (modals, palettes, dialogs).
//
// When a modal opens it (correctly) takes keyboard focus. When it
// closes, focus is left wherever the close action happened to be —
// often a button (the X) or `document.body` (after Esc / outside-
// click). The user then can't type into the terminal until they
// click it again.
//
// First-try implementation captured `document.activeElement` on
// mount and restored on unmount, but that only worked when the
// modal was opened by a shortcut from a focused terminal — opening
// via the cog button (or any non-terminal trigger) captured the
// button instead, and "restoring" put focus right back on the
// button, not the terminal. The daily-driver intent is unambiguous:
// after dismissing an overlay, you want to keep typing.
//
// Approach now: dispatch a window-level `skein:overlay-closed`
// event on unmount. `LiveTerminal` listens and, if it's the
// currently-visible terminal, refocuses its xterm. That makes
// every overlay-close path (Esc, X button, backdrop click,
// programmatic close) end up in the same place — the active
// terminal — regardless of where focus was when the overlay opened.
// Issue #33.

import { useEffect } from "react";

export const OVERLAY_CLOSED_EVENT = "skein:overlay-closed";

export function useFocusRestore(): void {
	useEffect(() => {
		return () => {
			window.dispatchEvent(new Event(OVERLAY_CLOSED_EVENT));
		};
	}, []);
}
