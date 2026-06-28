// Centralized list of App-owned keyboard shortcuts.
//
// `matchShortcut` is the single source of truth: the window-level
// keydown listener (App.tsx) dispatches on the action it returns, and
// xterm's per-terminal custom key handler (LiveTerminal.tsx) uses
// `isAppShortcut` (a thin wrapper) to decide whether to swallow the
// key so the byte never reaches the PTY (otherwise Alt+W would also
// delete-word in the shell, Alt+1..9 would echo, etc).
//
// Primary modifier:
//   - macOS:        ⌘ (Cmd). Frees ⌃ for terminal control codes the
//                   way Mac users expect (Terminal.app, iTerm, VS Code).
//   - Windows/Linux: Alt. Sits where ⌘ does for finger parity (#151),
//                   instead of a direct ⌘→Ctrl port that put everything
//                   on the pinky and shadowed terminal control codes
//                   (Ctrl+W/K/L/N). The mirror cost is that Alt+<letter>
//                   now shadows the terminal's Meta keys — a deliberate
//                   trade until user-customizable bindings (#150).
//
// The two schemes diverge in structure, not just the modifier key, so
// they're matched by separate per-platform functions below.
//
// e.code is layout-independent for letters/digits, so it works the same
// on US, Swedish, German, etc.

export const isMac =
	typeof navigator !== "undefined" && navigator.platform.toLowerCase().includes("mac");

// Secondary modifier for ROOM-navigation arrows on Windows/Linux
// (harness arrows are plain Alt). Alt+Ctrl per the chosen scheme (#151).
// Flip to "shift" if Alt+Ctrl+Arrow collides with AltGr (= Ctrl+Alt on
// Swedish/European layouts) or the Intel GPU screen-rotate hotkey on
// your machine — the hint labels track this constant automatically.
const WIN_ROOM_ARROW_MOD: "ctrl" | "shift" = "ctrl";

/** Glyph for hint copy: "⌘" on macOS, "Alt" on Windows/Linux. */
export const modLabel = isMac ? "⌘" : "Alt";

export type ShortcutAction =
	| "newRoom"
	| "closeRoom"
	| "palette"
	| "settings"
	| "addHarness"
	| "reloadWindow"
	| "nextRoom"
	| "prevRoom"
	| "nextHarness"
	| "prevHarness"
	| "nextAlertedRoom"
	| "prevAlertedRoom"
	| "nextAlertedHarness"
	| "prevAlertedHarness"
	| "fontInc"
	| "fontDec"
	| "jumpRoom";

export interface ShortcutMatch {
	action: ShortcutAction;
	/** 0-based room index; only set when `action === "jumpRoom"`. */
	roomIndex?: number;
}

const digitRoomIndex = (code: string): number | null =>
	/^Digit[1-9]$/.test(code) ? Number.parseInt(code.slice(5), 10) - 1 : null;

/** macOS scheme: ⌘ primary, ⇧ for the secondary (room) axis. Unchanged. */
function matchMac(e: KeyboardEvent): ShortcutMatch | null {
	if (!e.metaKey || e.ctrlKey || e.altKey) return null;

	if (e.shiftKey) {
		switch (e.code) {
			case "KeyH":
				return { action: "addHarness" };
			case "KeyR":
				return { action: "reloadWindow" };
			case "KeyJ":
				return { action: "prevAlertedRoom" };
			case "KeyL":
				return { action: "prevAlertedHarness" };
			case "Tab":
			case "ArrowLeft":
				return { action: "prevRoom" };
			case "ArrowRight":
				return { action: "nextRoom" };
			default:
				return null;
		}
	}

	switch (e.code) {
		case "KeyN":
			return { action: "newRoom" };
		case "KeyW":
			return { action: "closeRoom" };
		case "KeyK":
			return { action: "palette" };
		case "Comma":
			return { action: "settings" };
		case "KeyJ":
			return { action: "nextAlertedRoom" };
		case "KeyL":
			return { action: "nextAlertedHarness" };
		case "Tab":
			return { action: "nextRoom" };
		case "ArrowLeft":
			return { action: "prevHarness" };
		case "ArrowRight":
			return { action: "nextHarness" };
		case "Equal":
			return { action: "fontInc" };
		case "Minus":
			return { action: "fontDec" };
		default: {
			const idx = digitRoomIndex(e.code);
			return idx === null ? null : { action: "jumpRoom", roomIndex: idx };
		}
	}
}

/**
 * Windows/Linux scheme: Alt primary. Harness arrows are plain Alt;
 * room arrows are Alt + the secondary modifier (`WIN_ROOM_ARROW_MOD`).
 * Alt+Tab is OS-reserved, so room cycling is arrows + Alt+digits only.
 */
function matchWin(e: KeyboardEvent): ShortcutMatch | null {
	if (!e.altKey || e.metaKey) return null;

	// Room arrows live on the secondary axis.
	const roomMod = WIN_ROOM_ARROW_MOD === "ctrl" ? e.ctrlKey : e.shiftKey;
	const otherMod = WIN_ROOM_ARROW_MOD === "ctrl" ? e.shiftKey : e.ctrlKey;
	if ((e.code === "ArrowLeft" || e.code === "ArrowRight") && roomMod && !otherMod) {
		return { action: e.code === "ArrowLeft" ? "prevRoom" : "nextRoom" };
	}
	// Alt+Ctrl is otherwise unbound (reserved for the room axis above).
	if (e.ctrlKey) return null;

	if (e.shiftKey) {
		switch (e.code) {
			case "KeyH":
				return { action: "addHarness" };
			case "KeyR":
				return { action: "reloadWindow" };
			case "KeyJ":
				return { action: "prevAlertedRoom" };
			case "KeyL":
				return { action: "prevAlertedHarness" };
			default:
				return null;
		}
	}

	switch (e.code) {
		case "KeyN":
			return { action: "newRoom" };
		case "KeyW":
			return { action: "closeRoom" };
		case "KeyK":
			return { action: "palette" };
		case "Comma":
			return { action: "settings" };
		case "KeyJ":
			return { action: "nextAlertedRoom" };
		case "KeyL":
			return { action: "nextAlertedHarness" };
		case "ArrowLeft":
			return { action: "prevHarness" };
		case "ArrowRight":
			return { action: "nextHarness" };
		case "Equal":
			return { action: "fontInc" };
		case "Minus":
			return { action: "fontDec" };
		default: {
			const idx = digitRoomIndex(e.code);
			return idx === null ? null : { action: "jumpRoom", roomIndex: idx };
		}
	}
}

/** The app action a keydown maps to, or `null` if it isn't a shortcut. */
export const matchShortcut = (e: KeyboardEvent): ShortcutMatch | null =>
	isMac ? matchMac(e) : matchWin(e);

/** True when the event is a reserved app shortcut (xterm passthrough gate). */
export const isAppShortcut = (e: KeyboardEvent): boolean => matchShortcut(e) !== null;

// Human-readable hint glyphs, platform-aware. `roomSecGlyph` tracks
// WIN_ROOM_ARROW_MOD so the room-nav hints stay correct if it's flipped.
const roomSecGlyph = WIN_ROOM_ARROW_MOD === "ctrl" ? "Ctrl" : "⇧";

export const hints = {
	newRoom: `${modLabel} N`,
	addHarness: `${modLabel} ⇧ H`,
	closeRoom: `${modLabel} W`,
	reload: `${modLabel} ⇧ R`,
	nextAlertedRoom: `${modLabel} J`,
	prevAlertedRoom: `${modLabel} ⇧ J`,
	nextAlertedHarness: `${modLabel} L`,
	prevAlertedHarness: `${modLabel} ⇧ L`,
	nextHarness: `${modLabel} →`,
	nextRoom: isMac ? `${modLabel} Tab` : `${modLabel} ${roomSecGlyph} →`,
	roomNavDesc: isMac
		? "Next room (⇧ for previous, 1-9 for nth)"
		: `Next room (${modLabel} ${roomSecGlyph} ← previous, ${modLabel} 1-9 for nth)`,
	harnessNavDesc: isMac
		? "Next harness (⌘ ⇧ → for next room, ⌘ ← / ⌘ ⇧ ← for previous)"
		: `Next harness (${modLabel} ${roomSecGlyph} → next room, ${modLabel} ← / ${modLabel} ${roomSecGlyph} ← previous)`,
} as const;
