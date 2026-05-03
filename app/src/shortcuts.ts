// Centralized list of App-owned keyboard shortcuts.
//
// Both the window-level keydown listener (App.tsx) and xterm's
// per-terminal custom key handler (LiveTerminal.tsx) consult this:
// App acts on the combo, xterm returns false so the byte never
// reaches the PTY (otherwise Ctrl+W would also delete-word in the
// shell, Ctrl+1..9 would echo "1", etc).
//
// e.code is layout-independent for letters/digits, so it works the
// same on US, Swedish, German, etc. We use e.key only where it's
// already settled (font-size +/-).

export const isAppShortcut = (e: KeyboardEvent): boolean => {
	if (!e.ctrlKey || e.altKey || e.metaKey) return false;

	// Ctrl+Shift combos
	if (e.shiftKey) {
		if (e.code === "KeyH") return true; // add harness
		if (e.code === "Tab") return true; // previous session
		return false;
	}

	// Ctrl-only combos
	if (e.code === "KeyN") return true; // new session
	if (e.code === "KeyW") return true; // close session
	if (e.code === "KeyK") return true; // palette
	if (e.code === "Tab") return true; // next session
	if (/^Digit[1-9]$/.test(e.code)) return true; // jump to session N
	// Font size — already wired in App.tsx; listed here so xterm doesn't
	// also forward "=" / "+" / "-" to the PTY.
	if (e.code === "Equal" || e.code === "Minus") return true;
	return false;
};
