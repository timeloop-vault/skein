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
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import { isAppShortcut, isMac } from "./shortcuts.ts";

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
		// Take focus on mount so the user can immediately interact with
		// any prompt the spawned CLI prints (e.g. Claude Code's "is it
		// ok if I work in this folder?" arrow-key dialog).
		term.focus();

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
						if (cur) void invoke("pty_resize", { id: cur, rows: term.rows, cols: term.cols });
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
		};
	}, [mountKey]);

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
