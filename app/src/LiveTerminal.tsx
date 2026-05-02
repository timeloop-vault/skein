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
// Phase 4: when the child exits we keep the xterm and its scrollback
// alive, write a "[skein] x exited (N)" line + footer, and intercept the
// next keystroke. Enter spawns the user's default shell into the same
// xterm (and persists that as the harness's cmd via onCmdChange);
// R re-runs the command that just exited.

import { Channel, invoke } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";

type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

interface LiveTerminalProps {
	cmd: string[];
	cwd: string;
	// Stable identity for the spawn — used so React's StrictMode
	// double-invocation in dev doesn't spawn twice. Passing the
	// harness id is the natural choice.
	mountKey: string;
	fontSize: number;
	// Default shell argv (from `default_shell`). Used when the user
	// presses Enter on the post-exit prompt to drop into a usable shell.
	defaultShell: string[];
	// Persists a new cmd against this harness so a Skein restart
	// re-spawns the shell instead of the dead CLI.
	onCmdChange: (cmd: string[]) => void;
}

export const LiveTerminal = ({
	cmd,
	cwd,
	mountKey,
	fontSize,
	defaultShell,
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
		term.open(host);
		// If we're mounting into a hidden pane (e.g. an inactive session
		// at app boot), skip the initial fit. xterm's defaults (24×80) are
		// what we'll spawn the PTY with; the ResizeObserver tick that
		// fires when the pane becomes visible will refit and pty_resize.
		if (host.clientWidth > 0 && host.clientHeight > 0) {
			fit.fit();
		}
		termRef.current = term;
		fitRef.current = fit;
		// Take focus on mount so the user can immediately interact with
		// any prompt the spawned CLI prints (e.g. Claude Code's "is it
		// ok if I work in this folder?" arrow-key dialog).
		term.focus();

		// Phase 4 state: which command spawned most recently (for the
		// "[skein] x exited" message and for R-to-retry), and whether
		// we're showing the post-exit prompt.
		let lastCmd = cmd;
		let phase: "running" | "exited" = "running";

		// Clipboard bindings (Ctrl+Shift+C / V) plus the post-exit prompt
		// keys. Ctrl+C / Ctrl+V keep their terminal meaning while running;
		// Ctrl+Shift+C / Ctrl+Shift+V do editor-style copy/paste — same
		// convention as VS Code's terminal, gnome-terminal, kitty, etc.
		term.attachCustomKeyEventHandler((e) => {
			if (e.type !== "keydown") return true;

			if (phase === "exited") {
				if (e.key === "Enter") {
					const shell = defaultShellRef.current;
					if (shell.length > 0) {
						onCmdChangeRef.current(shell);
						void respawn(shell);
					}
					return false;
				}
				if (e.key === "r" || e.key === "R") {
					void respawn(lastCmd);
					return false;
				}
				// Swallow other keys while at the prompt — forwarding
				// them to a dead writer would error.
				return false;
			}

			if (!e.ctrlKey || !e.shiftKey) return true;
			if (e.code === "KeyC") {
				const sel = term.getSelection();
				if (sel) void writeText(sel);
				// Suppress xterm's default handling either way — sending the
				// raw Ctrl+Shift+C byte sequence to the PTY is rarely useful.
				return false;
			}
			if (e.code === "KeyV") {
				void readText().then((text) => {
					const id = ptyIdRef.current;
					if (text && id) void invoke("pty_write", { id, data: text });
				});
				return false;
			}
			return true;
		});

		const channel = new Channel<PtyEvent>();
		channel.onmessage = (ev) => {
			if (ev.kind === "data") {
				term.write(ev.chunk);
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
			// Stop forwarding keystrokes — the writer is gone, and we
			// want Enter/R to flow through the custom handler instead.
			dataDisposable?.dispose();
			dataDisposable = null;
			// Note: deliberately keep ptyIdRef pointing at the dead
			// manager entry. respawn pty_kills it before spawning a
			// fresh one, which evicts the leaked Windows reader thread.
			// Unmount cleanup also calls pty_kill so it doesn't leak.

			const program = lastCmd[0] ?? "child";
			const codeStr = code === null ? "?" : String(code);
			// \x1b[2m = dim, \x1b[1m = bold, \x1b[0m = reset.
			term.write(`\r\n\x1b[2m[skein] ${program} exited (${codeStr})\x1b[0m\r\n`);
			term.write(
				"\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell, \x1b[0;1mR\x1b[0;2m to retry.\x1b[0m\r\n",
			);
		};

		const startPty = async (cmdToSpawn: string[]) => {
			if (cancelled) return;
			lastCmd = cmdToSpawn;
			phase = "running";
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
					void invoke("pty_write", { id, data });
				});
				if (!resizeObserver) {
					resizeObserver = new ResizeObserver(() => {
						// Phase 3 guard: when the session goes display:none,
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
						if (cur) void invoke("pty_resize", { id: cur, rows: term.rows, cols: term.cols });
					});
					resizeObserver.observe(host);
				}
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				term.write(`\r\n\x1b[31m[skein] pty_spawn failed: ${msg}\x1b[0m\r\n`);
				// Reprompt — without this the user sees the error and has
				// no idea their input keys (Enter / R) still work.
				term.write(
					"\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell, \x1b[0;1mR\x1b[0;2m to retry.\x1b[0m\r\n",
				);
				phase = "exited";
			}
		};

		const respawn = async (cmdToSpawn: string[]) => {
			// Old PTY is already exited; pty_kill cleans the manager's
			// HashMap entry and is a no-op against the dead child. We
			// always call it, even if ptyIdRef was cleared by handleExit,
			// because the manager entry (with its leaked reader thread on
			// Windows ConPTY) is still parked on the old master.
			const oldId = ptyIdRef.current;
			if (oldId) void invoke("pty_kill", { id: oldId });
			ptyIdRef.current = null;

			// Hand a clean canvas to the next child:
			//   \x1b[?1049l — exit the alternate screen buffer if we
			//                 were left in it (some TUIs terminate
			//                 without restoring main screen).
			//   \x1b[2J     — clear the visible viewport (does NOT touch
			//                 main-screen scrollback).
			//   \x1b[H      — home the cursor.
			// Without this, Claude #2 enters its alt screen on top of
			// stale buffer content from Claude #1 and only repaints
			// dirty cells, so the user sees a mash-up until they type.
			term.write("\x1b[?1049l\x1b[2J\x1b[H");
			term.scrollToBottom();

			await startPty(cmdToSpawn);
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
		};
		// cmd/cwd/fontSize are stable for a given mountKey (cmd is mutated
		// via onCmdChange but the spawnedRef guard short-circuits before
		// any work happens; fontSize is reapplied by the effect below).
		// Listed only to satisfy useExhaustiveDependencies.
	}, [mountKey, cmd, cwd, fontSize]);

	// Live font-size changes: retune the existing terminal without
	// re-spawning the PTY. fit() recomputes rows/cols at the new cell
	// size; we then tell the PTY to match so the child sees the resize.
	useEffect(() => {
		const term = termRef.current;
		const fit = fitRef.current;
		const host = containerRef.current;
		if (!term || !fit) return;
		term.options.fontSize = fontSize;
		// Same hidden-host guard as the ResizeObserver: fitting a 0×0
		// host would squish scrollback to 1 col. Updating fontSize alone
		// is fine; the refit will happen on the next visibility tick.
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
