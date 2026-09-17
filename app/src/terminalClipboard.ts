// terminalClipboard — pure decision logic for LiveTerminal's copy/paste
// key handling (#158). Split out of LiveTerminal.tsx so the modifier
// matrix is testable without xterm or a DOM.
//
// Copy is custom everywhere because xterm needs to write the terminal's
// *selection* to the system clipboard, not the input bytes:
//   - macOS:   ⌘C            (Ctrl+C still sends SIGINT to the PTY)
//   - Windows/Linux: Ctrl+Shift+C always copies (Ctrl+C still sends
//     SIGINT). Windows ALSO gets Windows-Terminal-style smart Ctrl+C:
//     plain Ctrl+C copies when there's a live selection and clears it,
//     but with no selection it falls through untouched so \x03 still
//     reaches the PTY. Linux keeps plain Ctrl+C as SIGINT unconditionally
//     — no smart-Ctrl+C convention there, and overloading it would be a
//     surprise on a platform that doesn't do this anywhere else.
//
// Paste is native (the browser's `paste` event → xterm's own textarea →
// `term.onData`) everywhere EXCEPT the two combos below, which need a
// clipboard-manager fallback:
//   - Windows: plain Ctrl+V AND Ctrl+Shift+V. xterm's Keyboard.ts turns
//     Ctrl+<letter> into a C0 byte (Ctrl+V → \x16), forwards it to the
//     PTY and cancels the keydown — under WebView2 that frequently
//     starves the browser `paste` event before it ever fires, so plain
//     Ctrl+V is unreliable there too.
//   - Linux: only Ctrl+Shift+V. Plain Ctrl+V must keep reaching the PTY
//     as \x16 — Claude Code binds it to image paste on Linux.
//   - macOS: neither. Cmd+V is native and untouched.

export type ClipboardPlatform = "mac" | "windows" | "linux";

/** The subset of `KeyboardEvent` the decision needs — kept minimal so
 *  tests can build one by hand without a DOM. */
export interface ClipboardKeyLike {
	type: string;
	code: string;
	ctrlKey: boolean;
	shiftKey: boolean;
	altKey: boolean;
	metaKey: boolean;
}

export type ClipboardAction = "copy" | "paste";

/** Does this keydown mean "copy the xterm selection" or "paste from the
 *  OS clipboard"? `null` for everything else, including any event that
 *  isn't a `keydown` (xterm's `attachCustomKeyEventHandler` also sees
 *  `keyup`, which is never a shortcut here).
 *
 *  `hasSelection` only matters for Windows' smart plain-Ctrl+C — every
 *  other combo's verdict doesn't depend on whether anything is
 *  selected (Ctrl+Shift+C always copies, an empty copy is still a
 *  "copy" whose caller shows a "nothing selected" hint). */
export const decideClipboardAction = (
	e: ClipboardKeyLike,
	platform: ClipboardPlatform,
	hasSelection: boolean,
): ClipboardAction | null => {
	if (e.type !== "keydown") return null;

	if (e.code === "KeyC") {
		if (platform === "mac") {
			const macCopy = e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
			return macCopy ? "copy" : null;
		}
		const ctrlShiftC = e.ctrlKey && e.shiftKey && !e.metaKey && !e.altKey;
		if (ctrlShiftC) return "copy";
		if (platform === "windows") {
			const plainCtrlC = e.ctrlKey && !e.shiftKey && !e.metaKey && !e.altKey;
			if (plainCtrlC && hasSelection) return "copy";
		}
		return null;
	}

	if (e.code === "KeyV") {
		if (platform === "mac") return null; // native ⌘V handles it.
		const plainCtrlV = e.ctrlKey && !e.shiftKey && !e.metaKey && !e.altKey;
		const ctrlShiftV = e.ctrlKey && e.shiftKey && !e.metaKey && !e.altKey;
		if (platform === "windows") return plainCtrlV || ctrlShiftV ? "paste" : null;
		return ctrlShiftV ? "paste" : null; // linux: plain Ctrl+V stays \x16.
	}

	return null;
};

/** Hint text for a copy attempt that found nothing selected. Mouse
 *  tracking is what makes a plain drag go to the TUI instead of an
 *  xterm selection (Claude Code / opencode both enable it), so only
 *  mention the forced-selection chord when it's actually needed. */
export const emptySelectionHint = (
	platform: ClipboardPlatform,
	mouseTrackingOn: boolean,
): string => {
	if (!mouseTrackingOn) return "Nothing selected";
	const how = platform === "mac" ? "Option+drag" : "Shift+drag";
	return `Nothing selected — ${how} to select over the agent's UI`;
};
