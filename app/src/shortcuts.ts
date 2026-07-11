// Centralized list of App-owned keyboard shortcuts.
//
// `matchShortcut` is the single source of truth: the window-level keydown
// listener (App.tsx) dispatches on the action it returns, and xterm's
// per-terminal handler (LiveTerminal.tsx) uses `isAppShortcut` (a thin
// wrapper) to decide whether to swallow the key so the byte never reaches
// the PTY (otherwise Alt+W would delete-word in the shell, Alt+1..9 would
// echo, etc).
//
// The bindings live in ONE platform-agnostic table (`BINDINGS`), written
// in terms of *logical* modifiers. Only the per-OS `SCHEME` below changes
// what those logical modifiers resolve to:
//   - primary  — the main app modifier: ⌘ on macOS, Alt on Windows/Linux.
//                ⌘ keeps ⌃ free for terminal control codes; Alt sits where
//                ⌘ does for finger parity (#151) instead of a ⌘→Ctrl port
//                that shadowed Ctrl+W/K/L/N.
//   - roomAxis — the secondary modifier that selects room (vs harness)
//                arrow nav: ⇧ on macOS, Ctrl on Windows/Linux.
//   - tabRooms — whether Tab / Shift+Tab cycle rooms (off on Windows/Linux
//                where Alt+Tab is OS-reserved).
//
// e.code is layout-independent for letters/digits, so it works the same on
// US, Swedish, German, etc.

export const isMac =
	typeof navigator !== "undefined" && navigator.platform.toLowerCase().includes("mac");

// Secondary modifier for ROOM-navigation arrows on Windows/Linux (harness
// arrows are plain Alt). Alt+Ctrl per the chosen scheme (#151). Flip to
// "shift" if Alt+Ctrl+Arrow collides with AltGr (= Ctrl+Alt on Swedish/
// European layouts) or the Intel GPU screen-rotate hotkey — SCHEME and the
// hint labels both follow this constant automatically.
const WIN_ROOM_ARROW_MOD: "ctrl" | "shift" = "ctrl";

interface Scheme {
	/** Main app modifier. */
	primary: "meta" | "alt";
	/** Secondary modifier that selects room (vs harness) arrow navigation. */
	roomAxis: "ctrl" | "shift";
	/** Whether Tab / Shift+Tab cycle rooms (Alt+Tab is OS-reserved on Win). */
	tabRooms: boolean;
}

const SCHEME: Scheme = isMac
	? { primary: "meta", roomAxis: "shift", tabRooms: true }
	: { primary: "alt", roomAxis: WIN_ROOM_ARROW_MOD, tabRooms: false };

/** Glyph for hint copy: "⌘" on macOS, "Alt" on Windows/Linux. */
export const modLabel = isMac ? "⌘" : "Alt";

export type ShortcutAction =
	| "newRoom"
	| "closeRoom"
	| "palette"
	| "files"
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

// One binding. `primary` is always required; `shift` and `roomAxis` are
// optional extra modifiers (roomAxis resolves to SCHEME.roomAxis's key).
interface Binding {
	code: string;
	shift?: boolean;
	roomAxis?: boolean;
	action: ShortcutAction;
}

const BINDINGS: Binding[] = [
	// Primary-only actions.
	{ code: "KeyN", action: "newRoom" },
	{ code: "KeyW", action: "closeRoom" },
	{ code: "KeyK", action: "palette" },
	// E = explore. P stays free for #48's fuzzy quick-open later.
	{ code: "KeyE", action: "files" },
	{ code: "Comma", action: "settings" },
	{ code: "KeyJ", action: "nextAlertedRoom" },
	{ code: "KeyL", action: "nextAlertedHarness" },
	{ code: "Equal", action: "fontInc" },
	{ code: "Minus", action: "fontDec" },
	// Primary + Shift.
	{ code: "KeyH", shift: true, action: "addHarness" },
	{ code: "KeyR", shift: true, action: "reloadWindow" },
	{ code: "KeyJ", shift: true, action: "prevAlertedRoom" },
	{ code: "KeyL", shift: true, action: "prevAlertedHarness" },
	// Harness nav: primary + arrow.
	{ code: "ArrowLeft", action: "prevHarness" },
	{ code: "ArrowRight", action: "nextHarness" },
	// Room nav: primary + roomAxis + arrow.
	{ code: "ArrowLeft", roomAxis: true, action: "prevRoom" },
	{ code: "ArrowRight", roomAxis: true, action: "nextRoom" },
	// Room nav via Tab, where it's not OS-reserved.
	...(SCHEME.tabRooms
		? ([
				{ code: "Tab", action: "nextRoom" },
				{ code: "Tab", shift: true, action: "prevRoom" },
			] as Binding[])
		: []),
];

/** Physical Shift state a binding requires (roomAxis folds in when it's ⇧). */
const wantsShift = (b: Binding): boolean =>
	(b.shift ?? false) || (!!b.roomAxis && SCHEME.roomAxis === "shift");
/** Physical Ctrl state a binding requires (only when roomAxis is Ctrl). */
const wantsCtrl = (b: Binding): boolean => !!b.roomAxis && SCHEME.roomAxis === "ctrl";

/** The app action a keydown maps to, or `null` if it isn't a shortcut. */
export const matchShortcut = (e: KeyboardEvent): ShortcutMatch | null => {
	const primaryHeld = SCHEME.primary === "meta" ? e.metaKey : e.altKey;
	const wrongPrimary = SCHEME.primary === "meta" ? e.altKey : e.metaKey;
	if (!primaryHeld || wrongPrimary) return null;

	// Jump to room N: primary + digit, no other modifiers.
	if (!e.shiftKey && !e.ctrlKey) {
		const digit = /^Digit([1-9])$/.exec(e.code);
		if (digit?.[1]) return { action: "jumpRoom", roomIndex: Number(digit[1]) - 1 };
	}

	for (const b of BINDINGS) {
		if (b.code === e.code && wantsShift(b) === e.shiftKey && wantsCtrl(b) === e.ctrlKey) {
			return { action: b.action };
		}
	}
	return null;
};

/** True when the event is a reserved app shortcut (xterm passthrough gate). */
export const isAppShortcut = (e: KeyboardEvent): boolean => matchShortcut(e) !== null;

// Human-readable hint glyphs, platform-aware — derived from the same SCHEME
// so labels can't drift from the matcher.
const roomSecGlyph = SCHEME.roomAxis === "ctrl" ? "Ctrl" : "⇧";

export const hints = {
	newRoom: `${modLabel} N`,
	files: `${modLabel} E`,
	addHarness: `${modLabel} ⇧ H`,
	closeRoom: `${modLabel} W`,
	reload: `${modLabel} ⇧ R`,
	nextAlertedRoom: `${modLabel} J`,
	prevAlertedRoom: `${modLabel} ⇧ J`,
	nextAlertedHarness: `${modLabel} L`,
	prevAlertedHarness: `${modLabel} ⇧ L`,
	nextHarness: `${modLabel} →`,
	nextRoom: SCHEME.tabRooms ? `${modLabel} Tab` : `${modLabel} ${roomSecGlyph} →`,
	roomNavDesc: SCHEME.tabRooms
		? "Next room (⇧ for previous, 1-9 for nth)"
		: `Next room (${modLabel} ${roomSecGlyph} ← previous, ${modLabel} 1-9 for nth)`,
	harnessNavDesc: isMac
		? "Next harness (⌘ ⇧ → for next room, ⌘ ← / ⌘ ⇧ ← for previous)"
		: `Next harness (${modLabel} ${roomSecGlyph} → next room, ${modLabel} ← / ${modLabel} ${roomSecGlyph} ← previous)`,
} as const;
