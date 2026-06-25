// Centralized list of App-owned keyboard shortcuts.
//
// Both the window-level keydown listener (App.tsx) and xterm's
// per-terminal custom key handler (LiveTerminal.tsx) consult this:
// App acts on the combo, xterm returns false so the byte never
// reaches the PTY (otherwise Ctrl+W would also delete-word in the
// shell, Ctrl+1..9 would echo "1", etc).
//
// Mod = ⌘ on macOS, Ctrl on Windows/Linux. Using ⌘ on macOS frees
// Ctrl back up for terminal control codes (Ctrl+C, Ctrl+W in shells)
// the way Mac users expect — same convention as Terminal.app, iTerm,
// VS Code's terminal, etc.
//
// e.code is layout-independent for letters/digits, so it works the
// same on US, Swedish, German, etc. We use e.key only where it's
// already settled (font-size +/-).

export const isMac =
	typeof navigator !== "undefined" && navigator.platform.toLowerCase().includes("mac");

/** True when the platform's primary modifier is held (and only that one). */
export const isModKey = (e: KeyboardEvent): boolean =>
	isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;

/** Glyph for hint copy: "⌘" on macOS, "Ctrl" on Windows/Linux. */
export const modLabel = isMac ? "⌘" : "Ctrl";

export const isAppShortcut = (e: KeyboardEvent): boolean => {
	if (!isModKey(e) || e.altKey) return false;

	// Mod+Shift combos
	if (e.shiftKey) {
		if (e.code === "KeyH") return true; // add harness
		if (e.code === "KeyR") return true; // reload window (#121)
		if (e.code === "KeyJ") return true; // previous alerted room (#67)
		if (e.code === "KeyL") return true; // previous alerted harness (#67)
		if (e.code === "Tab") return true; // previous room
		if (e.code === "ArrowLeft") return true; // previous room (alias)
		if (e.code === "ArrowRight") return true; // next room (alias)
		return false;
	}

	// Mod-only combos
	if (e.code === "KeyN") return true; // new room
	if (e.code === "KeyW") return true; // close room
	if (e.code === "KeyK") return true; // palette
	if (e.code === "Comma") return true; // settings
	if (e.code === "KeyJ") return true; // next alerted room (#67)
	if (e.code === "KeyL") return true; // next alerted harness (#67)
	if (e.code === "Tab") return true; // next room
	if (e.code === "ArrowLeft") return true; // previous harness
	if (e.code === "ArrowRight") return true; // next harness
	if (/^Digit[1-9]$/.test(e.code)) return true; // jump to room N
	// Font size — already wired in App.tsx; listed here so xterm doesn't
	// also forward "=" / "+" / "-" to the PTY.
	if (e.code === "Equal" || e.code === "Minus") return true;
	return false;
};
