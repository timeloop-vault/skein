// terminalInteractions — the copy/paste key handling, the post-exit
// "Press Enter for shell" gate, Shift/Option+Enter → newline, and
// copy-on-select mouse wiring that `useTerminalSpawn`'s mount effect
// attaches to a live `Terminal`. Split out of LiveTerminal.tsx purely
// to keep that effect's file under the ~600-line house limit (#19); no
// behaviour change. `phase` and `cancelled` are owned by the caller
// (mutated by the PTY lifecycle) and read here through getters, since
// they're plain closure locals that can't cross a module boundary any
// other way.

import { invoke } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { Terminal } from "@xterm/xterm";
import { harnessInput } from "./harnessInput.ts";
import { isAppShortcut, isMac, isWindows } from "./shortcuts.ts";
import { decideClipboardAction, emptySelectionHint } from "./terminalClipboard.ts";
import type { ClipboardPlatform } from "./terminalClipboard.ts";

/// This platform, as far as the copy/paste key matrix cares — see
/// `terminalClipboard.ts`. Computed once; `navigator.platform` doesn't
/// change mid-session.
const clipboardPlatform: ClipboardPlatform = isMac ? "mac" : isWindows ? "windows" : "linux";

/// Transient hint durations (#158): long enough to read, short enough
/// not to linger over the TUI. Success is quick because it's expected;
/// failures/instructions get longer because they're the ones the user
/// needs to actually read.
const HINT_MS_SUCCESS = 1200;
const HINT_MS_INFO = 4000;

export interface TerminalInteractionsDeps {
	/// This harness's id — only used to note a user-driven paste
	/// (#380's `harnessInput.noteUserInput`), never for the phase/keys
	/// logic below.
	harnessId: string;
	/// Current post-exit phase, read (never written) here.
	getPhase: () => "running" | "exited";
	/// Whether the owning effect has torn down — settle guards for
	/// promises that resolve after teardown.
	isCancelled: () => boolean;
	defaultShellRef: { current: string[] };
	onCmdChangeRef: { current: (cmd: string[]) => void };
	ptyIdRef: { current: string | null };
	setHint: (hint: string | null) => void;
	hintTimerRef: { current: ReturnType<typeof setTimeout> | null };
	copyOnSelectRef: { current: boolean };
}

/** Attaches the custom key handler and the copy-on-select mouse
 *  listener to `term`/`host`. Returns a cleanup that undoes exactly
 *  what this function set up — mirrors the mount effect's own cleanup
 *  block for this concern, moved here unchanged. */
export function attachTerminalInteractions(
	term: Terminal,
	host: HTMLElement,
	deps: TerminalInteractionsDeps,
): () => void {
	const {
		harnessId,
		getPhase,
		isCancelled,
		defaultShellRef,
		onCmdChangeRef,
		ptyIdRef,
		setHint,
		hintTimerRef,
		copyOnSelectRef,
	} = deps;

	// #158: transient in-pane feedback for copy/paste — auto-dismisses,
	// a new hint replaces whatever's showing, and the timer is cleared
	// on respawn/unmount below so it can never setState after teardown.
	const showHint = (text: string, ms: number) => {
		if (hintTimerRef.current) clearTimeout(hintTimerRef.current);
		setHint(text);
		hintTimerRef.current = setTimeout(() => {
			setHint(null);
			hintTimerRef.current = null;
		}, ms);
	};

	// #158: the one path that ever writes a selection to the system
	// clipboard — shared by the Ctrl+C/⌘C keydown handling below and
	// the copy-on-select mouseup listener further down, so both get
	// identical hints and the same `cancelled` guard against a
	// promise settling after this effect has torn down.
	const copySelectionToClipboard = (sel: string) => {
		void writeText(sel)
			.then(() => {
				if (isCancelled()) return;
				showHint("Copied", HINT_MS_SUCCESS);
			})
			.catch((err: unknown) => {
				console.warn("[skein] clipboard copy failed:", err);
				if (isCancelled()) return;
				showHint("Copy failed", HINT_MS_INFO);
			});
	};

	// Copy binding plus the post-exit prompt keys.
	//
	// **Copy** is custom because xterm needs to write the *selection*
	// to the system clipboard, not the input bytes:
	// - macOS:   ⌘C                (Ctrl+C still sends SIGINT to the PTY)
	// - Win/Linux: Ctrl+Shift+C always copies (Ctrl+C still sends SIGINT)
	// - Windows ALSO gets Windows-Terminal-style smart plain Ctrl+C: it
	//   copies (and clears the selection) only when there's a live
	//   selection; with no selection it falls through untouched so
	//   \x03 still reaches the PTY. A harness is interrupted with Esc,
	//   not Ctrl+C, in this scheme — Ctrl+C only copies, and only when
	//   there's something selected; with nothing selected it still
	//   reaches the PTY as SIGINT in a shell or Claude's clear/exit.
	//   Linux keeps plain Ctrl+C as unconditional SIGINT — no
	//   smart-Ctrl+C convention there.
	// It's checked (and handled) BEFORE the post-exit-prompt gate below
	// so copying the final output still works once the harness has
	// exited — previously the branch was unreachable there (#158).
	//
	// **Copy on select** (#158, Settings → "Copy on select", default
	// true on every platform) is a separate mouseup listener below —
	// finishing a mouse selection copies it too, without waiting for
	// any of the chords above. It shares `copySelectionToClipboard`
	// with this handler but is otherwise independent: it never clears
	// the selection and never touches the keyboard event.
	//
	// **Paste** is native (the browser `paste` event on xterm's hidden
	// textarea → `term.onData` → our outer `pty_write` wiring) for
	// every combo except the two `decideClipboardAction` calls
	// "paste": Windows' plain-Ctrl+V and Ctrl+Shift+V (WebView2's
	// paste event is unreliable enough that xterm's native path often
	// never fires), and Linux's Ctrl+Shift+V (plain Ctrl+V stays a
	// \x16 byte to the PTY — Claude Code binds it to image paste
	// there). macOS is untouched; ⌘V is always native. For an
	// intercepted combo we `e.preventDefault()` and return false —
	// that's load-bearing, it's what stops the native `paste` event
	// from *also* firing — then read the OS clipboard ourselves and
	// hand the text to `term.paste()`, xterm's own paste path (so it
	// still gets bracketed-paste framing and a single `onData`
	// message, not a readText→pty_write bypass — that shape produced
	// the double-paste bugs in #4/#5, so it must not come back).
	term.attachCustomKeyEventHandler((e) => {
		if (e.type !== "keydown") return true;

		// Reserved app shortcuts: don't let xterm forward the byte to
		// the PTY. The window-level listener in App.tsx handles them.
		if (isAppShortcut(e)) return false;

		const clipboardAction = decideClipboardAction(e, clipboardPlatform, term.hasSelection());

		if (clipboardAction === "copy") {
			// Windows-Terminal-style smart Ctrl+C (#158): plain Ctrl+C —
			// as opposed to the always-copy Ctrl+Shift+C — only reaches
			// here because `decideClipboardAction` already confirmed a
			// live selection, so it clears it and prevents the default
			// SIGINT byte instead of the general copy path's "return
			// false is enough" (Ctrl+Shift+C has no native browser
			// default worth suppressing; plain Ctrl+C does, hence the
			// explicit preventDefault here specifically).
			const isWindowsSmartCtrlC =
				clipboardPlatform === "windows" && e.ctrlKey && !e.shiftKey && !e.metaKey && !e.altKey;
			const sel = term.getSelection();
			if (sel) {
				copySelectionToClipboard(sel);
				if (isWindowsSmartCtrlC) {
					term.clearSelection();
					e.preventDefault();
				}
			} else {
				// Empty selection — or it didn't register. Claude Code /
				// opencode both enable mouse tracking, which routes a
				// plain drag to the TUI instead of an xterm selection;
				// tell the user the forced-selection chord that gets
				// them past it, when it's actually relevant (#158).
				const mouseTrackingOn = term.modes.mouseTrackingMode !== "none";
				console.warn(
					`[skein] copy: nothing selected${mouseTrackingOn ? " (mouse tracking is on — force a selection with Shift+drag / Option+drag)" : ""}`,
				);
				showHint(emptySelectionHint(clipboardPlatform, mouseTrackingOn), HINT_MS_INFO);
			}
			// Suppress xterm's default handling either way — sending the
			// raw modifier byte sequence to the PTY is rarely useful.
			return false;
		}

		if (getPhase() === "exited") {
			if (e.key === "Enter") {
				const shell = defaultShellRef.current;
				if (shell.length > 0) {
					// onCmdChange propagates the new cmd up to App
					// state. App's HarnessBody derives mountKey from
					// cmd content, so this triggers an unmount +
					// remount and a clean shell spawns into a fresh
					// xterm. No respawn / reset logic to maintain
					// here — just hand off and let React do it.
					onCmdChangeRef.current(shell);
				}
				return false;
			}
			// Swallow other keys while at the prompt — forwarding
			// them to a dead writer would error. Paste combos are
			// swallowed here too, same as before #158.
			return false;
		}

		if (clipboardAction === "paste") {
			e.preventDefault();
			void readText()
				.then((text) => {
					if (isCancelled() || getPhase() !== "running" || !text) return;
					term.paste(text);
					// #380: a human-driven paste, same as a keystroke —
					// `sendPrompt`'s gap/retry checks need to see it.
					harnessInput.noteUserInput(harnessId);
				})
				.catch((err: unknown) => {
					console.warn("[skein] clipboard paste failed:", err);
					if (isCancelled()) return;
					showHint("Paste failed", HINT_MS_INFO);
				});
			return false;
		}

		// Issue #27: Shift+Enter / Option+Enter / Alt+Enter →
		// ESC + CR ("insert newline in prompt"). Conventional
		// modifier-Enter sequence read by Claude Code, opencode,
		// and most TUI prompt UIs; matches iTerm's "Option as
		// Meta" output. xterm.js's default sends a bare \r for
		// every modifier-Enter combo, so the harness can't
		// distinguish submit from newline without our help.
		// Plain Enter falls through to xterm and still submits.
		// Ctrl+Enter is intentionally untouched — no consistent
		// convention there.
		//
		// preventDefault is load-bearing: returning false skips
		// xterm's keydown processing including its own
		// preventDefault call, so without this the browser's
		// default textarea behaviour inserts a \n that xterm
		// then forwards through its input listener — the
		// harness sees ESC+CR (newline) followed by \n (which
		// claude / opencode treat as submit). Suppress the
		// default explicitly so only our ESC+CR reaches the PTY.
		if (e.key === "Enter" && (e.shiftKey || e.altKey) && !e.ctrlKey && !e.metaKey) {
			e.preventDefault();
			const id = ptyIdRef.current;
			if (id) void invoke("pty_write", { id, data: "\x1b\r" });
			return false;
		}
		return true;
	});

	// #158: copy-on-select (Settings → "Copy on select", default
	// true) — not xterm's `onSelectionChange`, which fires
	// continuously while a drag is in progress, so a selection is
	// copied exactly once, when it's finished: a plain drag, a
	// Shift/Option+drag forced over an agent's TUI, or a double/
	// triple-click word/line select. Shares `copySelectionToClipboard`
	// with the keyboard path above, so the hints and the `cancelled`
	// guard are identical either way. Runs in both phases (running
	// and exited) — there's no reason to gate copying scrollback the
	// harness already produced.
	//
	// Armed on `mousedown` *inside the host, capture phase* (so it
	// fires before xterm's own mousedown handling can stop
	// propagation) and consumed by a one-shot `mouseup` on
	// `document`, also capture phase. A drag can be released outside
	// the host — over a splitter, a neighbouring pane, even outside
	// the window — and xterm tracks the drag on `ownerDocument`
	// regardless of where the mouseup lands, so a host-only mouseup
	// listener misses exactly that case. Deliberately NOT a permanent
	// document listener: every mounted terminal (hidden rooms
	// included) would otherwise re-copy its stale selection on any
	// click anywhere in the app. Only armed for primary-button
	// (`button === 0`) presses — a right-click opens a context menu,
	// not a selection.
	let armedMouseUp: ((e: MouseEvent) => void) | null = null;
	const copySelectionIfAny = () => {
		if (!copyOnSelectRef.current) return;
		if (!term.hasSelection()) return;
		const sel = term.getSelection();
		if (sel) copySelectionToClipboard(sel);
	};
	const handleHostMouseDown = (e: MouseEvent) => {
		if (e.button !== 0) return;
		if (armedMouseUp) document.removeEventListener("mouseup", armedMouseUp, true);
		const onDocMouseUp = () => {
			document.removeEventListener("mouseup", onDocMouseUp, true);
			armedMouseUp = null;
			copySelectionIfAny();
		};
		armedMouseUp = onDocMouseUp;
		document.addEventListener("mouseup", onDocMouseUp, true);
	};
	host.addEventListener("mousedown", handleHostMouseDown, true);

	return () => {
		// #158: copy-on-select — the host mousedown listener always
		// comes off; the document mouseup only if a drag is still
		// mid-flight (armed but not yet fired) when this tears down.
		host.removeEventListener("mousedown", handleHostMouseDown, true);
		if (armedMouseUp) {
			document.removeEventListener("mouseup", armedMouseUp, true);
			armedMouseUp = null;
		}
		// #158: cancel the hint auto-dismiss so it can't setState after
		// this effect has torn down (mount-key respawn or true unmount).
		if (hintTimerRef.current) clearTimeout(hintTimerRef.current);
		hintTimerRef.current = null;
		setHint(null);
	};
}
