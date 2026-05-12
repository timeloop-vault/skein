// LiveTerminal — xterm.js bound to a Tauri-side PTY.
//
// Mounting spawns the child via `pty_spawn` and wires three flows:
//   - PTY → terminal (Channel<PtyEvent> from Rust → term.write / exit handler)
//   - terminal → PTY (term.onData → invoke "pty_write")
//   - resize → PTY (ResizeObserver → fit → invoke "pty_resize")
//
// Unmount kills the child. Hidden panes (display:none) keep their PTY
// alive — the resize/fit path is guarded against zero-size hosts so we
// never tell xterm or the child that the terminal shrank to 1×1.
//
// When the child exits we keep the xterm and its scrollback alive
// long enough to write a "[skein] x exited (N)" line + "Press Enter
// for shell." footer. Pressing Enter calls `onCmdChange(shell)`,
// which updates the harness's stored cmd. App.tsx's HarnessBody
// derives the LiveTerminal mountKey from the cmd content, so a cmd
// change triggers a React unmount + fresh remount: new xterm, new
// PTY, no alt-screen state to reset, no scrollback to preserve. This
// is the only path back to a usable pane after a TUI exits — there's
// no in-pane retry. Chapter 5 makes harnesses resume on Skein
// restart anyway, and `claude --resume <uuid>` from the shell covers
// "come back to my conversation" without the alt-screen handover
// bug class.

import { Channel, invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { harnessActivity } from "./harnessActivity.ts";
import { isAppShortcut, isMac } from "./shortcuts.ts";
import { OVERLAY_CLOSED_EVENT } from "./useFocusRestore.ts";

type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

interface LiveTerminalProps {
	cmd: string[];
	cwd: string;
	// Stable identity for the spawn — used so React's StrictMode
	// double-invocation in dev doesn't spawn twice. Passing the
	// harness id is the natural choice.
	mountKey: string;
	// Stable across cmd changes (mountKey churns when the user
	// picks "Enter for shell" — see App.tsx). Used as the key into
	// the activity store so a respawn under the same harness shows
	// up as a fresh `spawning → running` transition rather than a
	// new ghost record. Epic #50.
	harnessId: string;
	fontSize: number;
	// Default shell argv (from `default_shell`). Used when the user
	// presses Enter on the post-exit prompt to drop into a usable shell.
	defaultShell: string[];
	// True iff this terminal is the one the user can currently see
	// and interact with: its room is active, no picker is up in
	// front of it, and it's the room's active harness. We focus the
	// xterm whenever this flips true (or on mount with `visible:
	// true`) so keyboard-driven room/harness switches don't leave
	// focus stranded on document.body. Issue #22.
	visible: boolean;
	// Persists a new cmd against this harness so a Skein restart
	// re-spawns the shell instead of the dead CLI.
	onCmdChange: (cmd: string[]) => void;
}

export const LiveTerminal = ({
	cmd,
	cwd,
	mountKey,
	harnessId,
	fontSize,
	defaultShell,
	visible,
	onCmdChange,
}: LiveTerminalProps) => {
	const containerRef = useRef<HTMLDivElement>(null);
	// Track live-spawn state by mountKey so StrictMode's double effect
	// doesn't spawn twice, and so a re-mount with the same harness can
	// reuse the previous spawn id when we add reconnects later.
	const spawnedRef = useRef<string | null>(null);
	// Used by the clipboard key handler — needs the PTY id at keystroke
	// time, which the spawn .then sets later.
	const ptyIdRef = useRef<string | null>(null);
	// Refs to the xterm + fit addon so the font-size effect below can
	// retune them without re-spawning the PTY.
	const termRef = useRef<Terminal | null>(null);
	const fitRef = useRef<FitAddon | null>(null);

	// Sync the latest props into refs so the long-lived effect's closure
	// always reads current values. defaultShell starts empty and gets
	// hydrated by the async `default_shell` invoke; onCmdChange is
	// recreated on every App render. Refs avoid having to re-run the
	// PTY-owning effect on those changes.
	const defaultShellRef = useRef(defaultShell);
	defaultShellRef.current = defaultShell;
	const onCmdChangeRef = useRef(onCmdChange);
	onCmdChangeRef.current = onCmdChange;

	// Run only on mountKey changes. cmd / cwd / fontSize are consumed
	// once at first mount: cmd seeds the closure's programName /
	// startPty call; cwd is fixed for a harness; fontSize has its own
	// retune effect below. Listing them in the dep array (the obvious
	// fix to satisfy useExhaustiveDependencies) made every App render
	// produce a fresh cmd-array reference, which made the cleanup tear
	// down the live PTY and the next effect-run re-spawn with the same
	// `--session-id <uuid>`. Claude refuses to reclaim a session whose
	// .jsonl file already exists, so the existing harness died with
	// "Session ID is already in use." Suppress the lint instead.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see comment.
	useEffect(() => {
		const host = containerRef.current;
		if (!host) return;
		if (spawnedRef.current === mountKey) return;
		spawnedRef.current = mountKey;

		const term = new Terminal({
			fontFamily: '"JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
			fontSize,
			theme: {
				background: "#131418",
				foreground: "#e8e6df",
				cursor: "#c96442",
				selectionBackground: "#20232a",
			},
			cursorBlink: true,
			scrollback: 5000,
			allowProposedApi: true,
		});
		const fit = new FitAddon();
		term.loadAddon(fit);
		// Issue #23: Ink-based TUIs (Claude Code, opencode) compute
		// display widths using the `string-width` package, which uses
		// Unicode 11+ tables. xterm.js's default width calculation
		// uses Unicode 6 (circa 2010), so chars like → / ✓ / various
		// arrows are width-2 to Ink but width-1 to xterm. Every such
		// char drifts the cursor by one column, and Ink's incremental
		// streaming redraws stack on top of each other instead of
		// landing where the previous frame's content was — text and
		// dividers leak through. Submitting forces a full clear + redraw
		// so it self-heals on send, which is the smoking gun.
		// The Unicode 11 addon ships an updated wcwidth table that
		// matches what string-width sees. activeVersion = "11" engages
		// it (default stays "6" otherwise — addon must be both loaded
		// AND activated).
		term.loadAddon(new Unicode11Addon());
		term.unicode.activeVersion = "11";

		// Issue #24: Cmd-click (macOS) / Ctrl-click (Win/Linux) opens
		// URIs in the OS-registered handler. Two paths:
		//
		// 1. Plain-text URIs that appear in terminal output (e.g. claude
		//    printing a docs URL, gh printing a PR-create URL). Detected
		//    by WebLinksAddon's regex matcher. The default regex only
		//    catches http(s); we widen it to any well-formed
		//    `scheme://...`, so vscode://, slack://, file://, ssh://,
		//    etc. all work. The OS picks the handler app, so we don't
		//    need a per-scheme allow-list. The leading word-boundary +
		//    ASCII-only scheme guard avoids matching things like
		//    `std::vector` (`vector` isn't a valid URI authority).
		//
		//    The regex has *no* `g` flag — WebLinkProvider always
		//    appends `g` to whatever flags we pass (see
		//    @xterm/addon-web-links/src/WebLinkProvider.ts:60), so
		//    a global flag here would produce `'gg'` and throw
		//    SyntaxError. Letting it own the global flag matches its
		//    contract.
		//
		//    `mailto:` and other no-`://` URIs aren't supported via
		//    this provider — the addon's `isUrl` validator requires
		//    the matched text to start with `protocol://host`, and
		//    `mailto:` URLs have empty hosts. A custom link provider
		//    would be needed; deferred.
		//
		// 2. OSC 8 hyperlinks — the explicit terminal escape
		//    `\e]8;;url\e\\text\e]8;;\e\\` that `gh`, `ls --hyperlink`,
		//    etc. emit. Wired via `term.options.linkHandler`. xterm's
		//    default would call `window.open` which Tauri's WebKit
		//    refuses, so without this OSC 8 links are dead.
		//
		// Modifier check matches the rest of Skein: Cmd on mac, Ctrl
		// elsewhere (see shortcuts.ts). Without modifier, the click
		// falls through to xterm's normal selection behaviour.
		const uriRegex = /\b[a-zA-Z][a-zA-Z0-9+.-]+:\/\/[^\s()[\]{}"'<>\\^`|]+/;
		const handleUriClick = (event: MouseEvent, uri: string) => {
			const isModifierClick = isMac ? event.metaKey : event.ctrlKey;
			if (!isModifierClick) return;
			event.preventDefault();
			void openUrl(uri).catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] openUrl failed:", uri, msg);
			});
		};
		term.loadAddon(new WebLinksAddon(handleUriClick, { urlRegex: uriRegex }));
		term.options.linkHandler = {
			activate: (event, uri) => handleUriClick(event, uri),
		};

		term.open(host);
		// If we're mounting into a hidden pane (e.g. an inactive room
		// at app boot), skip the initial fit. xterm's defaults (24×80) are
		// what we'll spawn the PTY with; the ResizeObserver tick that
		// fires when the pane becomes visible will refit and pty_resize.
		if (host.clientWidth > 0 && host.clientHeight > 0) {
			fit.fit();
		}
		termRef.current = term;
		fitRef.current = fit;
		// Initial focus is handled by the `visible` effect below — that
		// effect fires on mount as well as on every visible-flip, so a
		// terminal mounting visible (e.g. fresh harness from picker, only
		// room at boot) still gets focus, but a terminal mounting hidden
		// (inactive room's pre-mounted harness) doesn't steal it.

		// Track whether we're showing the post-exit prompt, plus the
		// program name for the "[skein] x exited (N)" line.
		let phase: "running" | "exited" = "running";
		let programName = cmd[0] ?? "child";

		// Copy binding plus the post-exit prompt keys.
		//
		// **Copy** is custom because xterm needs to write the *selection*
		// to the system clipboard, not the input bytes:
		// - macOS:     ⌘C        (Ctrl+C still sends SIGINT to the PTY)
		// - Win/Linux: Ctrl+Shift+C (Ctrl+C still sends SIGINT)
		//
		// **Paste** is left to xterm.js's native paste handling. xterm
		// listens to the browser's `paste` event on its hidden textarea
		// and writes the bytes through `term.onData`, which our
		// outer wiring then forwards to the PTY. Adding our own Cmd+V
		// handler used to fire that path *plus* the native one for a
		// double-paste; see #5 / #4 — removing the custom branch fixes
		// both.
		term.attachCustomKeyEventHandler((e) => {
			if (e.type !== "keydown") return true;

			// Reserved app shortcuts: don't let xterm forward the byte to
			// the PTY. The window-level listener in App.tsx handles them.
			if (isAppShortcut(e)) return false;

			if (phase === "exited") {
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
				// them to a dead writer would error.
				return false;
			}

			const copyCombo = isMac
				? e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey
				: e.ctrlKey && e.shiftKey && !e.metaKey && !e.altKey;
			if (copyCombo && e.code === "KeyC") {
				const sel = term.getSelection();
				if (sel) void writeText(sel);
				// Suppress xterm's default handling either way — sending the
				// raw modifier byte sequence to the PTY is rarely useful.
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

		const channel = new Channel<PtyEvent>();
		channel.onmessage = (ev) => {
			if (ev.kind === "data") {
				term.write(ev.chunk);
				// Feed the activity model. Every PTY chunk is an
				// "output" signal — the store throttles internally
				// so we don't fire a React render per chunk. Epic #50.
				harnessActivity.recordOutput(harnessId);
			} else {
				handleExit(ev.code);
			}
		};

		let cancelled = false;
		let dataDisposable: { dispose(): void } | null = null;
		let resizeObserver: ResizeObserver | null = null;

		const handleExit = (code: number | null) => {
			if (cancelled) return;
			phase = "exited";
			harnessActivity.exited(harnessId, code);
			// Stop forwarding keystrokes — the writer is gone, and we
			// want Enter to flow through the custom handler instead.
			dataDisposable?.dispose();
			dataDisposable = null;
			// Note: deliberately keep ptyIdRef pointing at the dead
			// manager entry. respawn pty_kills it before spawning a
			// fresh one, which evicts the leaked Windows reader thread.
			// Unmount cleanup also calls pty_kill so it doesn't leak.

			const codeStr = code === null ? "?" : String(code);
			// \x1b[2m = dim, \x1b[1m = bold, \x1b[0m = reset.
			term.write(`\r\n\x1b[2m[skein] ${programName} exited (${codeStr})\x1b[0m\r\n`);
			term.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
		};

		const startPty = async (cmdToSpawn: string[]) => {
			if (cancelled) return;
			programName = cmdToSpawn[0] ?? "child";
			phase = "running";
			// Record the spawn before we await — gives a deterministic
			// `spawning` window in the activity store even when
			// pty_spawn is slow. recordOutput in the channel handler
			// will flip it to `running` on the first chunk. Epic #50.
			harnessActivity.spawned(harnessId);
			try {
				const id = await invoke<string>("pty_spawn", {
					cmd: cmdToSpawn,
					cwd,
					rows: term.rows,
					cols: term.cols,
					onEvent: channel,
				});
				if (cancelled) {
					void invoke("pty_kill", { id });
					return;
				}
				ptyIdRef.current = id;
				dataDisposable = term.onData((data) => {
					// Focus-in / focus-out escapes are sent by xterm
					// when the child enabled DECSET 1004 (Claude Code,
					// opencode both do). The child typically reacts
					// with a full screen redraw — those bytes come
					// back through channel.onmessage and would
					// otherwise count as "activity" and reset the
					// idle timer. Mute the activity window for this
					// harness so the induced redraw doesn't lie about
					// what the child is doing. Covers every focus
					// path: harness switch, alt+tab back to Skein,
					// modal-close focus return, click into pane. Epic
					// #50.
					if (data === "\x1b[I" || data === "\x1b[O") {
						harnessActivity.muteInducedOutput(harnessId);
					}
					void invoke("pty_write", { id, data });
				});
				if (!resizeObserver) {
					// Track the dims we last sent so we can skip the
					// pty_resize round-trip when nothing actually
					// changed. Most ResizeObserver fires on a hidden→
					// visible flip end up with the same rows/cols xterm
					// already had — and many TUIs (Claude Code, opencode)
					// react to SIGWINCH by repainting their entire screen,
					// which then comes back to us as PTY output and
					// counts as "activity" in the harnessActivity store.
					// The visible symptom before this guard: switching
					// to an idle background harness made its tab dot go
					// green for 8s before settling back to idle. Epic #50.
					let lastSentRows = term.rows;
					let lastSentCols = term.cols;
					resizeObserver = new ResizeObserver(() => {
						// Phase 3 guard: when the room goes display:none,
						// the host shrinks to 0×0 and the observer fires.
						// Fitting to that size would tell xterm + the child
						// that the terminal is 1×1, permanently squishing
						// whatever's already in the scrollback. Skip while
						// hidden — the next tick (visible again) refits.
						if (host.clientWidth === 0 || host.clientHeight === 0) return;
						try {
							fit.fit();
						} catch {
							// fit can throw during teardown when the host
							// element has been detached; ignore.
							return;
						}
						const cur = ptyIdRef.current;
						if (!cur) return;
						if (term.rows === lastSentRows && term.cols === lastSentCols) return;
						lastSentRows = term.rows;
						lastSentCols = term.cols;
						void invoke("pty_resize", { id: cur, rows: term.rows, cols: term.cols });
					});
					resizeObserver.observe(host);
				}
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				term.write(`\r\n\x1b[31m[skein] pty_spawn failed: ${msg}\x1b[0m\r\n`);
				// Reprompt — without this the user sees the error and has
				// no idea Enter still drops them into a shell.
				term.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
				phase = "exited";
				// pty_spawn never produced a child; treat as exited
				// so the status bar / tab dot don't sit on spawning.
				harnessActivity.exited(harnessId, null);
			}
		};

		void startPty(cmd);

		return () => {
			cancelled = true;
			dataDisposable?.dispose();
			resizeObserver?.disconnect();
			const id = ptyIdRef.current;
			if (id) void invoke("pty_kill", { id });
			term.dispose();
			termRef.current = null;
			fitRef.current = null;
			spawnedRef.current = null;
			ptyIdRef.current = null;
			// Drop the activity record so the store doesn't keep
			// growing across the app's lifetime. A respawn (mountKey
			// change for "Enter for shell") re-runs the effect and
			// re-records via spawned() above. Epic #50.
			harnessActivity.forget(harnessId);
		};
	}, [mountKey]);

	// Issue #22: focus the xterm whenever this pane becomes visible —
	// covers keyboard-driven room switches (Mod+1..9, palette,
	// Mod+Tab), harness-within-room switches, and the picker → pick
	// → newly-active-harness flow. The mount effect runs before this
	// one (declaration order), so termRef is populated by the time we
	// dereference it. We *don't* track focus on visible→false: hiding
	// the pane via display:none already drops focus naturally; trying
	// to "restore" focus elsewhere would fight whatever just received
	// it (modal, command palette, etc.).
	// Issue #33: when an overlay (Settings, command palette, etc.)
	// dismisses, useFocusRestore in the overlay fires
	// `skein:overlay-closed` on window. The currently-visible
	// terminal grabs focus so the user can keep typing — regardless
	// of where focus was before the overlay opened (it might have
	// been on a chrome button rather than a terminal, in which case
	// "restoring" the original focus would land back on the button).
	useEffect(() => {
		if (!visible) return;
		const onOverlayClosed = () => termRef.current?.focus();
		window.addEventListener(OVERLAY_CLOSED_EVENT, onOverlayClosed);
		return () => window.removeEventListener(OVERLAY_CLOSED_EVENT, onOverlayClosed);
	}, [visible]);

	useEffect(() => {
		if (visible) termRef.current?.focus();
	}, [visible]);

	// Live font-size changes: retune the existing terminal without
	// re-spawning the PTY. fit() recomputes rows/cols at the new cell
	// size; we then tell the PTY to match so the child sees the resize.
	//
	// Containment for #16: the harness column and per-harness wrapper
	// have `overflow: hidden` set (`.sk-harness-col` in styles.css and
	// the inline style in `App.tsx`). Without that, xterm's canvas
	// pushes the flex column taller when the font grows, fit reads the
	// stretched parent height, and the row count never decreases —
	// content overflows and stays overflowed. With containment in
	// place, fit reads the constrained parent and reduces rows
	// correctly.
	useEffect(() => {
		const term = termRef.current;
		const fit = fitRef.current;
		const host = containerRef.current;
		if (!term || !fit) return;
		term.options.fontSize = fontSize;
		if (!host || host.clientWidth === 0 || host.clientHeight === 0) return;
		try {
			fit.fit();
		} catch {
			return;
		}
		const id = ptyIdRef.current;
		if (id) void invoke("pty_resize", { id, rows: term.rows, cols: term.cols });
	}, [fontSize]);

	return <div ref={containerRef} style={{ width: "100%", height: "100%" }} />;
};
