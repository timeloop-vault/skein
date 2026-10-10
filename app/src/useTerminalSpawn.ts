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
import { backgroundTasks } from "./backgroundTasks.ts";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import { harnessInput } from "./harnessInput.ts";
import type { ShellClaimSink } from "./opencodeShellClaim.ts";
import { followOpencodeShell, type OpencodeAdapter } from "./opencodeShellFollow.ts";
import { bufferPtyInput } from "./ptyInputBuffer.ts";
import type { PtySpawnResult } from "./ptySpawnResult.ts";
import { shellClaim } from "./shellClaim.ts";
import { startupAdoption } from "./startupAdoption.ts";
import { subagents } from "./subagents.ts";
import { attachTerminalInteractions } from "./terminalInteractions.ts";
import { attachOutputGate } from "./terminalOutputVisibility.ts";
import { createXterm, type LinkPolicy } from "./terminalSetup.ts";
import { attachAdapters } from "./terminalSpawnAdapters.ts";
import { agentResolves } from "./terminalSpawnAgent.ts";
import { attachPtyInput, observeResize, registerInputTarget } from "./terminalSpawnInput.ts";
import type { HarnessKind } from "./types.ts";
import { useClaudeRepoint } from "./useClaudeRepoint.ts";

type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

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
	opencodeClaim: ShellClaimSink | undefined;
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
		opencodeClaim,
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

		const caps = HARNESS_KINDS[harnessKind].capabilities;
		const linkPolicy: LinkPolicy = { mouseClicksDisabled: false };
		const { term, fit } = createXterm(host, fontSize, caps.opensClickedLinks, linkPolicy);
		termRef.current = term;
		fitRef.current = fit;
		// #592: every xterm write goes through this gate (hidden = buffered).
		// Created before any other host observer so the reveal flush runs
		// before the refit.
		const { gate, detach: detachGate } = attachOutputGate(term, host);
		// #383 follow-up: xterm's IME composition (CJK, an emoji picker,
		// possibly a dead-key accent) calls `_finalizeComposition`
		// straight into `onData`, never `onKey` — the only place this
		// store sees it. Start and end fold to `userPaste` (→ `unknown`),
		// not a guess at what was composed: fails safe.
		const noteComposition = () => harnessInput.noteDraftEvent(harnessId, { type: "userPaste" });
		term.textarea?.addEventListener("compositionstart", noteComposition);
		term.textarea?.addEventListener("compositionend", noteComposition);
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
			harnessId,
			imagePaste: caps.imagePaste,
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
			// #490: a killed PTY can still deliver its shutdown redraw after
			// this effect's cleanup ran. Without this guard those chunks
			// land on the respawned harness's fresh activity record and flip
			// `spawning` → `running` before SessionStart can claim it.
			if (cancelled) return;
			if (ev.kind === "data") {
				gate.write(ev.chunk);
				// Feed the activity model (epic #50) with every chunk, gated
				// or not: the store throttles internally, and the chunk also
				// feeds the L2b pattern matcher's per-harness tail buffer.
				harnessActivity.recordOutput(harnessId, ev.chunk);
				shellFollow?.onOutput(); // #517
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
		const opencodeAdapter: { current: OpencodeAdapter | null } = { current: null };
		let shellFollow: ReturnType<typeof followOpencodeShell> | null = null;
		// #238: this harness's entry in the `harnessInput` seam (the
		// Nudge button today, #41's file drop later). Registered once
		// the PTY is live, unregistered on exit/unmount/respawn — a
		// dead or about-to-respawn harness must not still be a nudge
		// target.
		let detachInputTarget: (() => void) | null = null;

		const handleExit = (code: number | null) => {
			if (cancelled) return;
			phase = "exited";
			shellFollow?.dispose();
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
			gate.write(`\r\n\x1b[2m[skein] ${programName} exited (${codeStr})\x1b[0m\r\n`);
			gate.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
		};

		const agentResolvesFor = (cmdToSpawn: string[]) =>
			agentResolves({
				agent,
				cmdToSpawn,
				harnessKind,
				harnessId,
				cwd,
				out: gate,
				isCancelled: () => cancelled,
				onRefused: () => {
					phase = "exited";
				},
			});

		const startPty = async (cmdToSpawn: string[]) => {
			if (cancelled) return;
			programName = cmdToSpawn[0] ?? "child";
			shellClaim.noteSpawn(harnessId, harnessKind, cmdToSpawn[0]); // #318
			startupAdoption.forget(harnessId); // #539: a new process voids the old one's candidates
			if (!(await agentResolvesFor(cmdToSpawn))) return;
			if (cancelled) return;
			phase = "running";
			// Record the spawn before we await — gives a deterministic
			// `spawning` window in the activity store even when
			// pty_spawn is slow. recordOutput in the channel handler
			// will flip it to `running` on the first chunk. Epic #50.
			harnessActivity.spawned(harnessId);
			// #491: transcript rows older than this process say nothing about it.
			claudeVersionStore.markSpawned(harnessId, Date.now());
			// #484: catch xterm's reply to ConPTY's cursor query (portable-pty
			// 0.9 waits for it), which can arrive before pty_spawn resolves. If
			// unmounted meanwhile, the settle paths below dispose it.
			const early = bufferPtyInput(term);
			// #401: re-learned from each spawn. A var the user exports inside the
			// post-exit shell is invisible to Rust and not covered.
			linkPolicy.mouseClicksDisabled = false;
			try {
				const { id, injected, mouseClicksDisabled } = await invoke<PtySpawnResult>("pty_spawn", {
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
					early.dispose();
					void invoke("pty_kill", { id });
					return;
				}
				ptyIdRef.current = id;
				linkPolicy.mouseClicksDisabled = mouseClicksDisabled;
				harnessActivity.setInjected(harnessId, injected);
				// #238: publish this harness to the `harnessInput` seam now
				// that its PTY is live.
				detachInputTarget = registerInputTarget(term, harnessId, harnessKind, id, gate);
				// L2c attach point — see terminalSpawnAdapters.ts.
				const detach = attachAdapters({
					harnessId,
					roomId,
					cwd,
					harnessKind,
					sessionId,
					opencodePort,
					onSessionCaptured,
					onSessionFollowed,
					sessionIdRef,
					claudeAdapterRef,
				});
				if (detach && opencodePort !== undefined) {
					opencodeAdapter.current = { detach, port: opencodePort };
				}
				shellFollow?.dispose();
				shellFollow = followOpencodeShell(harnessKind, cmdToSpawn[0], {
					harnessId,
					roomId,
					cwd,
					ptyIdRef,
					sessionIdRef,
					adapter: opencodeAdapter,
					onSessionCaptured,
					onSessionFollowed,
					claim: opencodeClaim,
					setHint,
					hintTimerRef,
				});
				// No await between taking the buffer and attaching the real
				// listener, so no reply is lost or sent twice.
				const buffered = early.takeAndDispose();
				dataDisposable = attachPtyInput(term, harnessId, id);
				if (buffered.length > 0) void invoke("pty_write", { id, data: buffered.join("") });
				if (!resizeObserver) resizeObserver = observeResize(term, fit, host, ptyIdRef, gate);
			} catch (err: unknown) {
				early.dispose();
				const msg = err instanceof Error ? err.message : String(err);
				gate.write(`\r\n\x1b[31m[skein] pty_spawn failed: ${msg}\x1b[0m\r\n`);
				// Reprompt — else the user can't tell Enter opens a shell.
				gate.write("\x1b[2m[skein] Press \x1b[0;1mEnter\x1b[0;2m for shell.\x1b[0m\r\n");
				phase = "exited";
				// pty_spawn never produced a child; treat as exited
				// so the status bar / tab dot don't sit on spawning.
				harnessActivity.exited(harnessId, null);
			}
		};

		void startPty(cmd);

		return () => {
			cancelled = true;
			term.textarea?.removeEventListener("compositionstart", noteComposition);
			term.textarea?.removeEventListener("compositionend", noteComposition);
			dataDisposable?.dispose();
			resizeObserver?.disconnect();
			// L2c-1: detach before pty_kill so the adapter stops
			// reading the JSONL — Claude itself will flush a final
			// system row on exit and we don't need to react to it.
			claudeAdapterRef.current?.detach();
			claudeAdapterRef.current = null;
			shellFollow?.dispose();
			opencodeAdapter.current?.detach();
			detachInputTarget?.();
			// Key handling + copy-on-select cleanup — see
			// terminalInteractions.ts.
			detachInteractions();
			const id = ptyIdRef.current;
			if (id) void invoke("pty_kill", { id });
			detachGate();
			term.dispose();
			termRef.current = null;
			fitRef.current = null;
			spawnedRef.current = null;
			ptyIdRef.current = null;
			// Drop the activity record so the store doesn't keep growing
			// across the app's lifetime; a respawn (mountKey change for
			// "Enter for shell") re-records via spawned() above. Epic #50.
			harnessActivity.forget(harnessId);
			// #298: same lifetime — Rust rediscovers live subagents on the
			// next attach, so dropping the cache loses nothing.
			subagents.forget(harnessId);
			backgroundTasks.forget(harnessId);
			shellClaim.forget(harnessId);
			startupAdoption.forget(harnessId); // #539
		};
	}, [mountKey]);
	// #116 re-point effect: see useClaudeRepoint.ts.
	useClaudeRepoint({ sessionId, harnessId, roomId, cwd, claudeAdapterRef });
}
