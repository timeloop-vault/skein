// useTerminalSpawn — the xterm.js ↔ PTY binding at the heart of
// LiveTerminal: spawns/kills the child on the mountKey, attaches the
// L2c event adapters (Claude JSONL tail / opencode SSE), and registers
// the #238 nudge seam. Split out of LiveTerminal.tsx purely to keep
// the effect under the ~600-line house limit (#19); no behaviour
// change — this is the same mount effect (plus the #116 re-point
// effect that used to follow it in the component), just moved into a
// hook so LiveTerminal.tsx's own body stays short. See
// LiveTerminal.tsx's header comment for what the three PTY↔terminal
// flows are.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { useEffect } from "react";
import { listHarnessAgents, unknownAgentMessage, validateAgent } from "./agents.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { attachClaudeEvents, attachOpencodeEvents } from "./harnessEvents.ts";
import { harnessInput } from "./harnessInput.ts";
import { subagents } from "./subagents.ts";
import { attachTerminalInteractions } from "./terminalInteractions.ts";
import { createXterm } from "./terminalSetup.ts";
import type { HarnessKind } from "./types.ts";

type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

/// `pty_spawn`'s resolved value. `injected` mirrors whether #215's
/// config injection was non-empty for this spawn — see
/// `harnessActivity.injected` and the #238 nudge gate in
/// `harnessInput.ts`, the only consumers.
interface PtySpawnResult {
	id: string;
	injected: boolean;
}

export interface UseTerminalSpawnParams {
	cmd: string[];
	cwd: string;
	// Stable identity for the spawn — used so React's StrictMode
	// double-invocation in dev doesn't spawn twice.
	mountKey: string;
	harnessId: string;
	roomId: string;
	harnessKind: HarnessKind;
	sessionId: string | undefined;
	agent: string | undefined;
	opencodePort: number | undefined;
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	fontSize: number;
	containerRef: { current: HTMLDivElement | null };
	spawnedRef: { current: string | null };
	ptyIdRef: { current: string | null };
	termRef: { current: Terminal | null };
	fitRef: { current: FitAddon | null };
	hintTimerRef: { current: ReturnType<typeof setTimeout> | null };
	sessionIdRef: { current: string | undefined };
	claudeAdapterRef: { current: { detach: () => void; sessionId: string } | null };
	defaultShellRef: { current: string[] };
	onCmdChangeRef: { current: (cmd: string[]) => void };
	copyOnSelectRef: { current: boolean };
	setHint: (hint: string | null) => void;
}

/** Spawns the PTY for a (cmd, mountKey) and tears it down on unmount /
 *  mountKey change; also re-points the L2c-1 Claude adapter when
 *  `sessionId` moves under an already-running PTY (#116). Call at the
 *  position `LiveTerminal`'s own mount effect used to sit — the two
 *  `useEffect`s below preserve that original declaration order (the
 *  re-point effect always ran right after the mount effect). */
export function useTerminalSpawn(params: UseTerminalSpawnParams): void {
	const {
		cmd,
		cwd,
		mountKey,
		harnessId,
		roomId,
		harnessKind,
		sessionId,
		agent,
		opencodePort,
		onSessionCaptured,
		onSessionFollowed,
		fontSize,
		containerRef,
		spawnedRef,
		ptyIdRef,
		termRef,
		fitRef,
		hintTimerRef,
		sessionIdRef,
		claudeAdapterRef,
		defaultShellRef,
		onCmdChangeRef,
		copyOnSelectRef,
		setHint,
	} = params;

	// Run only on mountKey changes. cmd / cwd / fontSize are consumed
	// once at first mount: cmd seeds the closure's programName /
	// startPty call; cwd is fixed for a harness; fontSize has its own
	// retune effect (in LiveTerminal.tsx). Listing them in the dep
	// array (the obvious fix to satisfy useExhaustiveDependencies) made
	// every App render produce a fresh cmd-array reference, which made
	// the cleanup tear down the live PTY and the next effect-run
	// re-spawn with the same `--session-id <uuid>`. Claude refuses to
	// reclaim a session whose .jsonl file already exists, so the
	// existing harness died with "Session ID is already in use."
	// Suppress the lint instead.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see comment.
	useEffect(() => {
		const host = containerRef.current;
		if (!host) return;
		if (spawnedRef.current === mountKey) return;
		spawnedRef.current = mountKey;

		const { term, fit } = createXterm(host, fontSize);
		termRef.current = term;
		fitRef.current = fit;
		// Initial focus is handled by the `visible` effect in
		// LiveTerminal.tsx — that effect fires on mount as well as on
		// every visible-flip, so a terminal mounting visible (e.g.
		// fresh harness from picker, only room at boot) still gets
		// focus, but a terminal mounting hidden (inactive room's
		// pre-mounted harness) doesn't steal it.

		// Track whether we're showing the post-exit prompt, plus the
		// program name for the "[skein] x exited (N)" line.
		let phase: "running" | "exited" = "running";
		let programName = cmd[0] ?? "child";

		// Key handling (copy/paste, post-exit prompt, Shift+Enter) and
		// copy-on-select — see terminalInteractions.ts. `phase` and
		// `cancelled` are read there through getters since they're
		// plain closure locals owned by this effect.
		const detachInteractions = attachTerminalInteractions(term, host, {
			getPhase: () => phase,
			isCancelled: () => cancelled,
			defaultShellRef,
			onCmdChangeRef,
			ptyIdRef,
			setHint,
			hintTimerRef,
			copyOnSelectRef,
		});

		const channel = new Channel<PtyEvent>();
		channel.onmessage = (ev) => {
			if (ev.kind === "data") {
				term.write(ev.chunk);
				// Feed the activity model. Every PTY chunk is an
				// "output" signal — the store throttles internally
				// so we don't fire a React render per chunk. Epic
				// #50. The chunk is also fed into the per-harness
				// tail buffer the L2b pattern matcher (sudo / [y/n]
				// / Press Enter prompts on non-adapter harness
				// kinds) reads from.
				harnessActivity.recordOutput(harnessId, ev.chunk);
			} else {
				handleExit(ev.code);
			}
		};

		let cancelled = false;
		let dataDisposable: { dispose(): void } | null = null;
		let resizeObserver: ResizeObserver | null = null;
		// L2c-1: when a Claude harness has a pre-allocated sessionId
		// (chapter 5), attach the JSONL adapter so the dot reflects
		// `last-prompt` rows directly instead of waiting for the L2a
		// idle heuristic to time out. `null` for every other case
		// (non-claude kinds, claude without sessionId — picker
		// fallback). Cleaned up alongside pty_kill below. Lives in
		// `claudeAdapterRef` (not a local) so the #116 step two re-point
		// effect below can detach/reattach it without re-running this
		// whole spawn effect.
		// L2c-2: same shape for opencode. Adapter SSE-subscribes to
		// opencode's embedded server on the pre-allocated port, plus
		// captures the auto-allocated sessionID via the SSE
		// `session.created` event (chapter 5 phase 2b's sqlite poll
		// stays as fallback in App.tsx).
		let detachOpencodeAdapter: (() => void) | null = null;
		// #238: this harness's entry in the `harnessInput` seam (the
		// Nudge button today, #41's file drop later). Registered once
		// the PTY is live, unregistered on exit/unmount/respawn — a
		// dead or about-to-respawn harness must not still be a nudge
		// target.
		let detachInputTarget: (() => void) | null = null;

		const handleExit = (code: number | null) => {
			if (cancelled) return;
			phase = "exited";
			harnessActivity.exited(harnessId, code);
			detachInputTarget?.();
			detachInputTarget = null;
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

		/** Refuse the spawn when the harness names an agent the CLI no
		 *  longer has (#247).
		 *
		 *  Here rather than at pick time *as well as* at pick time: a room
		 *  restored from sqlite names an agent chosen weeks ago, and only
		 *  one of the four spawn paths fails loudly on its own. Claude
		 *  refuses a fresh spawn, but `claude --resume` and both opencode
		 *  paths accept a dead name and quietly run as something else —
		 *  #176's category, and invisible in a TUI where the warning
		 *  scrolls past. Returning false costs one CLI probe (~0.35 s) and
		 *  only for a harness that names an agent at all.
		 *
		 *  Verdicts other than `unknown` spawn: a degraded list cannot
		 *  prove a name is gone, and refusing on a CLI that would not run
		 *  would strand every harness in the app.
		 *
		 *  Gated on the argv actually carrying the flag, because the
		 *  record outlives the process it described: a harness swapped to
		 *  a shell by "Enter for shell" keeps `agent` set, and a dead
		 *  agent name must not stop the user getting a shell. That is a
		 *  question about *this* argv, not an attempt to recover a
		 *  decision from it — the name still comes from the record. */
		const agentResolves = async (cmdToSpawn: string[]): Promise<boolean> => {
			if (!agent || !cmdToSpawn.includes("--agent")) return true;
			const verdict = validateAgent(agent, await listHarnessAgents(harnessKind, cwd));
			if (cancelled) return false;
			if (verdict.kind === "unverified") {
				console.warn(`[skein] could not verify agent "${agent}": ${verdict.why}`);
			}
			if (verdict.kind !== "unknown") return true;
			term.write(`\r\n\x1b[31m[skein] ${unknownAgentMessage(agent, harnessKind)}\x1b[0m\r\n`);
			// Same footer every other dead-harness path writes, and for the
			// same reason: without it the pane is a wall of red with no
			// visible way forward.
			term.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
			phase = "exited";
			harnessActivity.exited(harnessId, null);
			return false;
		};

		const startPty = async (cmdToSpawn: string[]) => {
			if (cancelled) return;
			programName = cmdToSpawn[0] ?? "child";
			if (!(await agentResolves(cmdToSpawn))) return;
			if (cancelled) return;
			phase = "running";
			// Record the spawn before we await — gives a deterministic
			// `spawning` window in the activity store even when
			// pty_spawn is slow. recordOutput in the channel handler
			// will flip it to `running` on the first chunk. Epic #50.
			harnessActivity.spawned(harnessId);
			try {
				const { id, injected } = await invoke<PtySpawnResult>("pty_spawn", {
					cmd: cmdToSpawn,
					cwd,
					rows: term.rows,
					cols: term.cols,
					// #213: the backend mints the room's review token and
					// puts SKEIN_REVIEW_URL / _TOKEN / SKEIN_ROOM_ID /
					// SKEIN_HARNESS_ID into the child's environment. The
					// harness id is what makes an agent's reply
					// attributable to one harness rather than to "an
					// agent" — a room can have several.
					roomId,
					harnessId,
					// #215: which agent CLI this is, so the backend can
					// append `--plugin-dir` / set `OPENCODE_CONFIG` and
					// the review tools are there without the user
					// configuring anything. Sent as the harness *kind*
					// rather than inferred from `cmd`, because the two
					// legitimately disagree once the user has swapped a
					// command (the post-exit "Enter for shell" path).
					kind: harnessKind,
					onEvent: channel,
				});
				if (cancelled) {
					void invoke("pty_kill", { id });
					return;
				}
				ptyIdRef.current = id;
				harnessActivity.setInjected(harnessId, injected);
				// #238: publish this harness to the `harnessInput` seam now
				// that its PTY is live. `paste`/`bracketedPaste` read xterm
				// state directly; `submit` is a separate `pty_write` of a
				// bare "\r" after the paste, per the seam's contract.
				detachInputTarget = harnessInput.register(harnessId, {
					paste: (text) => term.paste(text),
					bracketedPaste: () => term.modes.bracketedPasteMode,
					submit: () => {
						void invoke("pty_write", { id, data: "\r" });
					},
				});
				// L2c-1 attach point: after PTY is alive, hook into the
				// Claude session log for authoritative running/waiting
				// signals. Only fires for Claude harnesses that own a
				// session uuid (chapter 5 `--session-id` pre-allocation).
				// The translator marks the activity store authoritative
				// once Rust confirms attach; until then L2a keeps
				// ticking, so a slow attach is a graceful degradation.
				if (harnessKind === "claude" && sessionIdRef.current) {
					const attachedSessionId = sessionIdRef.current;
					claudeAdapterRef.current = {
						detach: attachClaudeEvents(harnessId, roomId, attachedSessionId, cwd),
						sessionId: attachedSessionId,
					};
				}
				// L2c-2: attach the opencode SSE adapter when we have a
				// port (App allocated one via pick_free_port before the
				// spawn argv was finalized). Without a port the adapter
				// can't know where to subscribe — graceful fallback to
				// L2a + the sqlite-poll session-id capture.
				if (harnessKind === "opencode" && opencodePort !== undefined) {
					detachOpencodeAdapter = attachOpencodeEvents(
						harnessId,
						roomId,
						cwd,
						opencodePort,
						sessionId,
						onSessionCaptured,
						// #116: live, not the `sessionId` closed over above —
						// this harness's session id can change under a
						// long-lived adapter (`/new`, a `/sessions` pick).
						() => sessionIdRef.current,
						onSessionFollowed,
					);
				}
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
				// Separate hook for "did the user actually press a
				// key in this terminal?" — used by L5a notification
				// gating to tell real work cycles apart from startup
				// banner cycles, and (#86) to tell whether a keystroke
				// answered a permission dialog. onKey is the right
				// primitive for both: onData fires for *anything* the
				// terminal sends to the child, including auto-responses
				// to queries like `\x1b[6n` (cursor position) and
				// `\x1b[5n` (device status). Treating those as input was
				// the bug that lit up every room on Skein restart, and
				// would just as wrongly clear a permission dialog nobody
				// answered. `key` is the exact bytes this keystroke sends
				// to the child — the same string `onData` would carry for
				// it — so `recordInput` can classify it without a second
				// copy of the escape-sequence logic.
				// Caveat: onKey doesn't fire for paste — if the user
				// pastes without ever typing, their first task-idle
				// transition won't bump, and pasting an answer into a
				// permission dialog won't clear it either. Corner-case
				// false negative we'll address with a paste listener if
				// it matters in practice.
				term.onKey(({ key }) => {
					harnessActivity.recordInput(harnessId, key);
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
			// L2c-1: detach before pty_kill so the adapter stops
			// reading the JSONL — Claude itself will flush a final
			// system row on exit and we don't need to react to it.
			claudeAdapterRef.current?.detach();
			claudeAdapterRef.current = null;
			detachOpencodeAdapter?.();
			detachInputTarget?.();
			// Key handling + copy-on-select cleanup — see
			// terminalInteractions.ts.
			detachInteractions();
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
			// #298: same lifetime as the activity record — the Rust
			// side rediscovers live subagents fresh on the next
			// attach, so nothing is lost by dropping the cache here.
			subagents.forget(harnessId);
		};
	}, [mountKey]);

	// #116: re-point the Claude JSONL adapter when `sessionId` changes
	// under an already-running PTY — Claude's own `/clear` or in-tool
	// `/resume` moving to a different conversation, reported via
	// App.tsx's `skein://harness-session-start` listener updating the
	// harness record, which flows back down here as a new prop. The PTY
	// itself is untouched: only the tail target moves. A no-op when no
	// adapter is attached (PTY not live yet, or a non-Claude harness) —
	// the mount effect's own attach (above) picks up the current
	// sessionId whenever it eventually runs.
	useEffect(() => {
		const current = claudeAdapterRef.current;
		if (!current) return;
		if (typeof sessionId !== "string" || sessionId === current.sessionId) return;
		current.detach();
		claudeAdapterRef.current = {
			detach: attachClaudeEvents(harnessId, roomId, sessionId, cwd),
			sessionId,
		};
	}, [sessionId, harnessId, roomId, cwd, claudeAdapterRef]);
}
