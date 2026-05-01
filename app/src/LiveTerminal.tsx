// LiveTerminal — xterm.js bound to a Tauri-side PTY.
//
// Mounting spawns the child via `pty_spawn` and wires three flows:
//   - PTY → terminal (Channel<string> from Rust → term.write)
//   - terminal → PTY (term.onData → invoke "pty_write")
//   - resize → PTY (ResizeObserver → fit → invoke "pty_resize")
//
// Unmount kills the child. Phase 1 keeps it that simple — closing a
// harness ends the process, no scrollback persistence, no reconnect.

import { Channel, invoke } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";

interface LiveTerminalProps {
	cmd: string[];
	cwd: string;
	// Stable identity for the spawn — used so React's StrictMode
	// double-invocation in dev doesn't spawn twice. Passing the
	// harness id is the natural choice.
	mountKey: string;
}

export const LiveTerminal = ({ cmd, cwd, mountKey }: LiveTerminalProps) => {
	const containerRef = useRef<HTMLDivElement>(null);
	// Track live-spawn state by mountKey so StrictMode's double effect
	// doesn't spawn twice, and so a re-mount with the same harness can
	// reuse the previous spawn id when we add reconnects later.
	const spawnedRef = useRef<string | null>(null);
	// Used by the clipboard key handler — needs the PTY id at keystroke
	// time, which the spawn .then sets later.
	const ptyIdRef = useRef<string | null>(null);

	useEffect(() => {
		const host = containerRef.current;
		if (!host) return;
		if (spawnedRef.current === mountKey) return;
		spawnedRef.current = mountKey;

		const term = new Terminal({
			fontFamily: '"JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
			fontSize: 13,
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
		fit.fit();
		// Take focus on mount so the user can immediately interact with
		// any prompt the spawned CLI prints (e.g. Claude Code's "is it
		// ok if I work in this folder?" arrow-key dialog).
		term.focus();

		// Clipboard bindings. Ctrl+C / Ctrl+V keep their terminal meaning
		// (SIGINT and literal 0x16); Ctrl+Shift+C / Ctrl+Shift+V do the
		// editor-style copy/paste — same convention as VS Code's terminal,
		// gnome-terminal, kitty, etc.
		term.attachCustomKeyEventHandler((e) => {
			if (e.type !== "keydown" || !e.ctrlKey || !e.shiftKey) return true;
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

		const channel = new Channel<string>();
		channel.onmessage = (chunk) => term.write(chunk);

		let ptyId: string | null = null;
		let cancelled = false;
		let dataDisposable: { dispose(): void } | null = null;
		let resizeObserver: ResizeObserver | null = null;

		invoke<string>("pty_spawn", {
			cmd,
			cwd,
			rows: term.rows,
			cols: term.cols,
			onOutput: channel,
		})
			.then((id) => {
				if (cancelled) {
					// LiveTerminal was unmounted before the spawn returned.
					// Kill the child we just made and bail.
					void invoke("pty_kill", { id });
					return;
				}
				ptyId = id;
				ptyIdRef.current = id;

				dataDisposable = term.onData((data) => {
					void invoke("pty_write", { id, data });
				});

				resizeObserver = new ResizeObserver(() => {
					try {
						fit.fit();
					} catch {
						// fit can throw during teardown when the host
						// element has been detached; ignore.
						return;
					}
					void invoke("pty_resize", { id, rows: term.rows, cols: term.cols });
				});
				resizeObserver.observe(host);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				term.write(`\r\n\x1b[31mskein: pty_spawn failed: ${msg}\x1b[0m\r\n`);
			});

		return () => {
			cancelled = true;
			dataDisposable?.dispose();
			resizeObserver?.disconnect();
			if (ptyId) {
				void invoke("pty_kill", { id: ptyId });
			}
			term.dispose();
			spawnedRef.current = null;
			ptyIdRef.current = null;
		};
		// cmd/cwd are stable for a given mountKey (we set them once when
		// the harness is created and never mutate them), so listing them
		// here is just to satisfy useExhaustiveDependencies — the effect
		// only actually re-fires when mountKey changes.
	}, [mountKey, cmd, cwd]);

	return <div ref={containerRef} style={{ width: "100%", height: "100%" }} />;
};
