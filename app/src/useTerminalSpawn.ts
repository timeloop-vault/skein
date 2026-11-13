// useTerminalSpawn — the xterm.js ↔ PTY binding at the heart of
// LiveTerminal: on each mountKey it asks the daemon to start the harness or
// attach to its running process, attaches the L2c event adapters, and
// registers the #238 nudge seam; unmount only detaches (#731). Split out of
// LiveTerminal.tsx (#19); see its header for the PTY↔terminal flows.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { FitAddon } from "@xterm/addon-fit";
import type { Terminal } from "@xterm/xterm";
import { useEffect } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity } from "./harnessActivity.ts";
import { defaultEnsureDeps, detachHarness, ensureRunning, type PtyEvent } from "./harnessEnsure.ts";
import { restartFor } from "./harnessMounts.ts";
import type { ShellClaimSink } from "./opencodeShellClaim.ts";
import { followOpencodeShell, type OpencodeAdapter } from "./opencodeShellFollow.ts";
import { createSidecarDeathRecord, watchSidecarExit } from "./opencodeSidecarExit.ts";
import { sendPhaseInput } from "./phaseInput.ts";
import { bufferPtyInput } from "./ptyInputBuffer.ts";
import { forget as forgetLatency, recordEcho } from "./ptyLatency.ts";
import { shellClaim } from "./shellClaim.ts";
import { exitedBanner, startFailedBanner } from "./terminalBanners.ts";
import { applyEnsureResult, forgetViewState } from "./terminalEnsureApply.ts";
import { attachTerminalInteractions } from "./terminalInteractions.ts";
import { attachOutputGate } from "./terminalOutputVisibility.ts";
import { repaintPty } from "./terminalRepaint.ts";
import { createXterm, type LinkPolicy } from "./terminalSetup.ts";
import { attachAdapters } from "./terminalSpawnAdapters.ts";
import { agentResolves } from "./terminalSpawnAgent.ts";
import {
	attachCompositionNote,
	attachPtyInput,
	observeResize,
	registerInputTarget,
} from "./terminalSpawnInput.ts";
import type { HarnessKind, LaunchMode } from "./types.ts";
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
	onShellRequestRef: { current: (argv: string[]) => void };
	/** The stored record's tool argv + launch mode: the daemon's expectation (#752). */
	expectCmd: string[];
	expectLaunch: LaunchMode;
	copyOnSelectRef: { current: boolean };
	setHint: (hint: string | null) => void;
}

/** Asks the daemon to start (or attach to) the harness for a (cmd, mountKey)
 *  and detaches from it on unmount / mountKey change, never killing it; also re-points the L2c-1 Claude adapter when
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
		onShellRequestRef,
		copyOnSelectRef,
		setHint,
		expectCmd,
		expectLaunch,
	} = params;

	// Run only on mountKey changes. cmd / cwd / fontSize are consumed once at
	// first mount (fontSize has its own retune effect); listing them re-ran the
	// effect on every App render's fresh cmd array. Suppress the lint instead.
	// biome-ignore lint/correctness/useExhaustiveDependencies: see comment.
	useEffect(() => {
		const host = containerRef.current;
		if (!host) return;
		if (spawnedRef.current === mountKey) return;
		spawnedRef.current = mountKey;

		const caps = HARNESS_KINDS[harnessKind].capabilities;
		const linkPolicy: LinkPolicy = { mouseClicksDisabled: false };
		const { term, fit } = createXterm(host, fontSize, caps, linkPolicy, cwd);
		termRef.current = term;
		fitRef.current = fit;
		// #592: every xterm write goes through this gate (hidden = buffered).
		// Created before any other host observer so the reveal flush runs
		// before the refit.
		const { gate, detach: detachGate } = attachOutputGate(term, host);
		const detachComposition = attachCompositionNote(term, harnessId); // #383
		// Initial focus: the `visible` effect in LiveTerminal.tsx.

		// Post-exit prompt state, and the program name for the exit line.
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
			onShellRequestRef,
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
				// Keep the mirror's last-output stamp fresh with every chunk,
				// gated or not (the daemon hears the PTY itself, #732).
				harnessActivity.recordOutput(harnessId, ev.chunk);
				shellFollow?.onOutput(); // #517
			} else {
				handleExit(ev.code);
			}
		};

		let cancelled = false;
		let dataDisposable: { dispose(): void } | null = null;
		let resizeObserver: ResizeObserver | null = null;
		// L2c adapters: see attachAdapters (Claude's lives in `claudeAdapterRef`
		// so the #116 re-point effect can swap it).
		const opencodeAdapter: { current: OpencodeAdapter | null } = { current: null };
		let shellFollow: ReturnType<typeof followOpencodeShell> | null = null;
		// #238: this harness's `harnessInput` entry; live PTY only, never a dead one.
		let detachInputTarget: (() => void) | null = null;
		let stopOtherAdapter: (() => void) | null = null; // Hermes capture (#624) + opencode v2 SSE (#677), not #517's v1 opencodeAdapter; unmount only
		let sidecarGeneration: number | undefined; // #676
		// #731: this mount's attachment (detached on cleanup) and pending repaint.
		// `restart` is read now, before any await; the key is recorded only when
		// the ensure call is dispatched, so a cancelled StrictMode run keeps it.
		let attached: { attachId: number | null | undefined } | null = null;
		let cancelRepaint: (() => void) | null = null;
		const restart = restartFor(harnessId, mountKey);
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
			detachInputTarget?.();
			detachInputTarget = null;
			// The writer is gone: stop forwarding keys so Enter reaches our handler.
			dataDisposable?.dispose();
			dataDisposable = null;
			// Keep ptyIdRef on the dead entry; the next ensure replaces it.
			gate.write(exitedBanner(programName, code));
			gate.write(sidecarDied.take()); // #676: "" unless the sidecar died
		};

		const startPty = async (cmdToSpawn: string[]) => {
			if (cancelled) return;
			programName = cmdToSpawn[0] ?? "child";
			shellClaim.noteSpawn(harnessId, harnessKind, cmdToSpawn[0]); // #318
			const resolved = await agentResolves({
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
			if (!resolved || cancelled) return;
			phase = "running";
			const spawnStartedAt = Date.now();
			// #484: catch xterm's reply to ConPTY's cursor query (portable-pty
			// 0.9 waits for it), which can arrive before the ensure call resolves.
			// If unmounted meanwhile, the settle paths below dispose it.
			const early = bufferPtyInput(term);
			// #401: re-learned from each attach. A var the user exports inside the
			// post-exit shell is invisible to Rust and not covered.
			linkPolicy.mouseClicksDisabled = false;
			try {
				// #731: the daemon spawns from the stored record (or reuses the running
				// process); #730: it probes the opencode CLI, starts the sidecar and
				// swaps the argv itself. A failure comes back as this call's rejection.
				const result = await ensureRunning(
					{
						roomId,
						harnessId,
						rows: term.rows,
						cols: term.cols,
						envProfile,
						restart,
						mountKey,
						expectCmd,
						expectLaunch,
						onEvent: channel,
					},
					defaultEnsureDeps(() => cancelled),
				);
				if (!result || cancelled) {
					early.dispose();
					// Detach, never kill: the daemon decides when the process stops.
					if (result) detachHarness(harnessId, result.attachId);
					return;
				}
				const { id, hermesHooks, mouseClicksDisabled, started } = result;
				attached = { attachId: result.attachId };
				ptyIdRef.current = id;
				if (result.exited) {
					// The `exit` event comes through the channel (exit line + shell
					// prompt); nothing live to feed, so register no input/adapter/repaint.
					early.dispose();
					return;
				}
				const { sidecarGeneration: generation, port } = applyEnsureResult({
					result,
					harnessId,
					opencodePort,
					spawnStartedAt,
				});
				sidecarGeneration = generation;
				linkPolicy.mouseClicksDisabled = mouseClicksDisabled;
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
					freshProcess: started, // #336: a re-attached process may be mid-turn
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
				// On a re-attach the buffered cursor reply is not for the running process.
				if (started && buffered.length > 0) {
					void invoke("pty_write", { id, data: buffered.join("") });
				}
				if (!resizeObserver) resizeObserver = observeResize(term, fit, host, ptyIdRef, gate);
				// #731: a re-attach starts blank; a one-row resize and back repaints.
				if (!started) cancelRepaint = repaintPty(term, id, harnessId);
			} catch (err: unknown) {
				early.dispose();
				if (cancelled) return;
				gate.write(startFailedBanner(err));
				phase = "exited";
				// The daemon produced no child: tell it, so the status bar / tab
				// dot don't sit on spawning.
				sendPhaseInput(harnessId, { kind: "startFailed" });
			}
		};

		void startPty(cmd);

		return () => {
			cancelled = true;
			cancelRepaint?.();
			detachComposition();
			dataDisposable?.dispose();
			resizeObserver?.disconnect();
			// L2c-1: detach the adapter first so it stops reading the JSONL;
			// the process itself keeps running (#731) and a later mount re-attaches.
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
			// Detach, never kill (#731): the daemon stops a harness when its record
			// goes away or its room is archived; restart is the next mount's flag.
			if (attached) detachHarness(harnessId, attached.attachId);
			detachGate();
			term.dispose();
			termRef.current = null;
			fitRef.current = null;
			spawnedRef.current = null;
			ptyIdRef.current = null;
			forgetViewState(harnessId);
		};
	}, [mountKey]);
	// #116 re-point effect: see useClaudeRepoint.ts.
	useClaudeRepoint({ sessionId, harnessId, roomId, cwd, claudeAdapterRef });
}
