// useTerminalSpawn — the xterm.js ↔ PTY binding at the heart of
// LiveTerminal: spawns/kills the child on the mountKey, attaches the
// L2c event adapters, and registers the #238 nudge seam. Split out of
// LiveTerminal.tsx (#19); see its header for the PTY↔terminal flows.

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
import { createSidecarDeathRecord, watchSidecarExit } from "./opencodeSidecarExit.ts";
import { prepareOpencodeSpawn, realPrepareDeps } from "./opencodeV2Spawn.ts";
import { bufferPtyInput } from "./ptyInputBuffer.ts";
import { forget as forgetLatency, recordEcho } from "./ptyLatency.ts";
import { type PtyEvent, ptySpawn } from "./ptySpawnCall.ts";
import { shellClaim } from "./shellClaim.ts";
import { forgetSpawnedProfile, recordSpawnedProfile } from "./spawnedEnvProfile.ts";
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
	/** #615: read at spawn time only (effect deps are [mountKey]). */
	envProfile: string | undefined;
	/** #627: scopes Hermes session capture to this harness's profile. */
	hermesProfile: string | undefined;
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
		envProfile,
		hermesProfile,
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
	// once at first mount (fontSize has its own retune effect). Listing
	// them made every App render's fresh cmd array tear down the live PTY
	// and respawn with the same `--session-id <uuid>`, which Claude
	// refuses ("Session ID is already in use."). Suppress the lint instead.
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
		// Initial focus: the `visible` effect in LiveTerminal.tsx.

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
				recordEcho(ptyIdRef.current, performance.now()); // #639
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
		// L2c-1: a Claude harness with a pre-allocated sessionId gets the
		// JSONL adapter (see attachAdapters); it lives in `claudeAdapterRef`
		// so the #116 re-point effect can swap it without a respawn.
		// L2c-2: opencode's SSE adapter subscribes on the pre-allocated port
		// and captures the sessionID from `session.created`.
		const opencodeAdapter: { current: OpencodeAdapter | null } = { current: null };
		let shellFollow: ReturnType<typeof followOpencodeShell> | null = null;
		// #238: this harness's `harnessInput` entry; live PTY only, never a dead one.
		let detachInputTarget: (() => void) | null = null;
		let stopOtherAdapter: (() => void) | null = null; // Hermes capture (#624) + opencode v2 SSE (#677), not #517's v1 opencodeAdapter; unmount only
		let sidecarGeneration: number | undefined; // #676
		const sidecarDied = createSidecarDeathRecord((s) => gate.write(s));
		const stopSidecarWatch =
			harnessKind === "opencode"
				? watchSidecarExit(harnessId, () => sidecarGeneration, sidecarDied.note)
				: null;

		const handleExit = (code: number | null) => {
			if (cancelled) return;
			phase = "exited";
			shellFollow?.dispose();
			if (ptyIdRef.current) forgetLatency(ptyIdRef.current); // #639
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
			gate.write(sidecarDied.take()); // #676: "" unless the sidecar died
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
			if (!(await agentResolvesFor(cmdToSpawn)) || cancelled) return;
			phase = "running";
			// Record the spawn before we await: a deterministic `spawning`
			// window; the first output chunk flips it to `running`. Epic #50.
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
				// #675/#676: probes the version, and for v2 starts the sidecar.
				const prep = await prepareOpencodeSpawn({
					kind: harnessKind,
					cmd: cmdToSpawn,
					roomId,
					harnessId,
					cwd,
					sessionId,
					agent,
					envProfile,
					setOpencodeVersion: opencodeClaim?.setOpencodeVersion,
					setSessionId: onSessionCaptured,
					isCancelled: () => cancelled,
				});
				if (!prep) {
					early.dispose();
					return;
				}
				sidecarGeneration = prep.sidecarGeneration;
				const port = prep.v2 ? undefined : opencodePort; // #677: v2 attaches by sidecar generation
				const {
					id,
					injected,
					hermesHooks,
					mouseClicksDisabled,
					envProfile: appliedProfile,
				} = await ptySpawn({
					cmd: prep.argv,
					cwd,
					rows: term.rows,
					cols: term.cols,
					roomId,
					harnessId,
					kind: harnessKind,
					envProfile,
					onEvent: channel,
					sidecarGeneration,
				});
				if (cancelled) {
					early.dispose();
					void invoke("pty_kill", { id });
					return;
				}
				ptyIdRef.current = id;
				linkPolicy.mouseClicksDisabled = mouseClicksDisabled;
				recordSpawnedProfile(harnessId, appliedProfile ?? null);
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
					program: cmdToSpawn[0],
					hermesProfile,
					hermesHooks,
					sessionId,
					opencodePort: port,
					opencodeSidecarGeneration: sidecarGeneration,
					onSessionCaptured,
					onSessionFollowed,
					sessionIdRef,
					claudeAdapterRef,
				});
				if (detach && port !== undefined) {
					opencodeAdapter.current = { detach, port };
				} else if (detach && (harnessKind === "hermes" || harnessKind === "opencode"))
					stopOtherAdapter = detach;
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
				if (sidecarGeneration !== undefined && !cancelled) {
					void realPrepareDeps.stop(harnessId, sidecarGeneration).catch(() => {});
				}
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
			stopOtherAdapter?.();
			stopSidecarWatch?.();
			detachInputTarget?.();
			// Key handling + copy-on-select cleanup — see
			// terminalInteractions.ts.
			detachInteractions();
			const id = ptyIdRef.current;
			if (id) forgetLatency(id); // #639
			if (id) void invoke("pty_kill", { id });
			detachGate();
			term.dispose();
			termRef.current = null;
			fitRef.current = null;
			spawnedRef.current = null;
			ptyIdRef.current = null;
			// Drop the activity record; a respawn re-records via spawned().
			harnessActivity.forget(harnessId);
			// #298: same lifetime — Rust rediscovers live subagents on the
			// next attach, so dropping the cache loses nothing.
			subagents.forget(harnessId);
			backgroundTasks.forget(harnessId);
			shellClaim.forget(harnessId);
			startupAdoption.forget(harnessId); // #539
			forgetSpawnedProfile(harnessId); // #615
		};
	}, [mountKey]);
	// #116 re-point effect: see useClaudeRepoint.ts.
	useClaudeRepoint({ sessionId, harnessId, roomId, cwd, claudeAdapterRef });
}
