// #625: the binding between Hermes shell hooks and the activity store
// (L2c-3). The decisions live in `hermesHookAdapter.ts`; this file applies
// its effects, owns the Rust action-extraction attach, and runs the
// launch stand-in (see `LAUNCH_POLL_MS`). Attached only when `pty_spawn`
// reported `hermesHooks: true`.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { guardChannelHandler } from "./harnessEventsShared.ts";
import { harnessInput } from "./harnessInputRegistry.ts";
import { markClaimed, releaseHermesCapture } from "./hermesCapture.ts";
import {
	type HermesAdapterState,
	type HermesEffect,
	type HermesHookEvent,
	initialHermesState,
	reduceHermesHook,
} from "./hermesHookAdapter.ts";
import {
	applyHermesWork,
	clearHermesWork,
	endSubagent,
	type HermesWorkEvent,
	startSubagent,
} from "./hermesWork.ts";
import type { ScreenPhase } from "./screenPhase.ts";

export const HERMES_HOOK_EVENT = "skein://harness-hermes-hook";

/// Hermes fires no hook at launch (the first is at the first prompt), so
/// the 15 s launch-silent watchdog would degrade every idle-at-launch
/// Hermes. While `spawning`, the rendered `ready` status line (two reads
/// in a row, like the screen-phase tick) stands in as the launch signal.
const LAUNCH_POLL_MS = 500;

export interface HermesBindingDeps {
	call: (cmd: string, args: Record<string, unknown>) => Promise<unknown>;
	readScreen: (harnessId: string) => ScreenPhase;
	setInterval: (fn: () => void, ms: number) => unknown;
	clearInterval: (h: unknown) => void;
}

const defaultDeps: HermesBindingDeps = {
	call: (cmd, args) => invoke(cmd, args),
	readScreen: (id) => harnessInput.readScreenPhaseNow(id),
	setInterval: (fn, ms) => setInterval(fn, ms),
	clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
};

export interface HermesBindingParams {
	harnessId: string;
	roomId: string;
	hermesProfile?: string | null | undefined;
	/** Session id bound at spawn (a resume), else undefined. */
	boundSessionId?: string | undefined;
	/** The harness's current session id (live). */
	getSessionId: () => string | undefined;
	/** Same path hermesCapture feeds; used for the id on_session_start reports. */
	onSessionCaptured?: ((sessionId: string) => void) | undefined;
}

export interface HermesBinding {
	handle(ev: HermesHookEvent): void;
	dispose(): void;
}

const warn = (what: string, harnessId: string) => (err: unknown) => {
	const msg = err instanceof Error ? err.message : String(err);
	console.warn(`[skein] ${what} failed for ${harnessId}:`, msg);
};

export function createHermesBinding(
	p: HermesBindingParams,
	deps: HermesBindingDeps = defaultDeps,
): HermesBinding {
	const { harnessId } = p;
	let state: HermesAdapterState = initialHermesState;
	let attachedFor: string | undefined;
	let disposed = false;

	harnessActivity.attachAuthoritativeSource(harnessId);

	const attachActions = (sessionId: string | null) => {
		if (!sessionId || sessionId === attachedFor) return;
		// A session known at spawn was resumed; its prior spend is not this turn's.
		const resumed = attachedFor === undefined && sessionId === p.boundSessionId;
		// The old session's delegations can no longer report their ends.
		if (attachedFor !== undefined) clearHermesWork(harnessId);
		attachedFor = sessionId;
		const args = {
			harnessId,
			roomId: p.roomId,
			sessionId,
			hermesProfile: p.hermesProfile ?? null,
			resumed,
		};
		void deps.call("hermes_actions_attach", args).catch(warn("hermes_actions_attach", harnessId));
		// #628: outstanding delegated work (subagents + background processes).
		const onEvent = new Channel<HermesWorkEvent>();
		onEvent.onmessage = guardChannelHandler(harnessId, "hermes_work", (e) => {
			if (!disposed && attachedFor === sessionId) applyHermesWork(harnessId, e);
		});
		void deps
			.call("hermes_work_attach", { ...args, onEvent })
			.catch(warn("hermes_work_attach", harnessId));
	};
	attachActions(p.boundSessionId ?? null);

	// Launch stand-in: see LAUNCH_POLL_MS.
	let ready = 0;
	const poll = deps.setInterval(() => {
		const phase = harnessActivity.get(harnessId)?.phase;
		if (disposed || phase !== "spawning") {
			if (phase !== "spawning") deps.clearInterval(poll);
			return;
		}
		ready = deps.readScreen(harnessId) === "waiting" ? ready + 1 : 0;
		if (ready >= 2) {
			deps.clearInterval(poll);
			harnessActivity.noteLaunchSignal(harnessId, TRANSITION_SOURCE.L2c3HermesReady);
		}
	}, LAUNCH_POLL_MS);

	const apply = (e: HermesEffect) => {
		switch (e.kind) {
			case "sessionStarted":
				harnessActivity.noteLaunchSignal(harnessId, TRANSITION_SOURCE.L2c3HermesSessionStart);
				if (e.sessionId) {
					// The hook's id is authoritative: claim it and stop the
					// poll so no other pending harness (or a late tick) can
					// take or overwrite it.
					markClaimed(e.sessionId);
					releaseHermesCapture(harnessId);
					if (e.changed) p.onSessionCaptured?.(e.sessionId);
				}
				attachActions(e.sessionId);
				return;
			case "running":
				harnessActivity.setRunningFromAdapter(harnessId, TRANSITION_SOURCE.L2c3HermesRunning);
				return;
			case "waiting":
				if (e.interrupted) {
					harnessActivity.setWaitingFromAdapter(harnessId, TRANSITION_SOURCE.L2c3HermesInterrupted);
				} else {
					// #628: defers while delegated work is outstanding.
					harnessActivity.awaitingPromptFromAdapter(harnessId, TRANSITION_SOURCE.L2c3HermesEndTurn);
				}
				return;
			case "subagentStart":
				startSubagent(harnessId, e.agentId, e.description, false);
				return;
			case "subagentEnd":
				endSubagent(harnessId, e.agentId);
				return;
			case "permissionOpen":
				harnessActivity.setPermissionFromAdapter(
					harnessId,
					TRANSITION_SOURCE.L2c3HermesPermission,
					e.toolName,
				);
				return;
			case "permissionAnswered":
				harnessActivity.clearPermission(harnessId, TRANSITION_SOURCE.L2c3HermesPermissionAnswered);
				return;
			case "permissionAnsweredIdle":
				// Main turn already over: nothing more will arrive, so settle
				// to waiting (from `permission`, hence not notifiable).
				harnessActivity.setWaitingFromAdapter(
					harnessId,
					TRANSITION_SOURCE.L2c3HermesPermissionAnsweredIdle,
				);
				return;
			case "turnEnded":
				void deps
					.call("hermes_actions_turn_end", { harnessId })
					.catch(warn("hermes_actions_turn_end", harnessId));
				return;
		}
	};

	return {
		handle(ev) {
			if (disposed) return;
			harnessActivity.adapterDelivered(harnessId);
			const out = reduceHermesHook(state, ev, { currentSessionId: p.getSessionId() });
			state = out.state;
			for (const e of out.effects) apply(e);
		},
		dispose() {
			if (disposed) return;
			disposed = true;
			deps.clearInterval(poll);
			clearHermesWork(harnessId);
			harnessActivity.releasePermission(harnessId, TRANSITION_SOURCE.AdapterDetached);
			harnessActivity.detachAuthoritativeSource(harnessId);
			void deps
				.call("hermes_actions_detach", { harnessId })
				.catch(warn("hermes_actions_detach", harnessId));
			void deps
				.call("hermes_work_detach", { harnessId })
				.catch(warn("hermes_work_detach", harnessId));
		},
	};
}

// One app-level listener dispatching by harness id (the shape of
// useHarnessHookEvents), installed with the first binding and removed with
// the last.
const bindings = new Map<string, HermesBinding>();
let unlisten: Promise<UnlistenFn> | null = null;

export const dispatchHermesHook = (ev: HermesHookEvent): void => {
	bindings.get(ev.harnessId)?.handle(ev);
};

/// Attach the Hermes hook adapter for a PTY spawned with working hooks.
/// Returns the detach (idempotent). A newer attach for the same harness
/// replaces the older.
export function attachHermesEvents(p: HermesBindingParams, deps?: HermesBindingDeps): () => void {
	bindings.get(p.harnessId)?.dispose();
	const binding = createHermesBinding(p, deps);
	bindings.set(p.harnessId, binding);
	unlisten ??= listen<HermesHookEvent>(HERMES_HOOK_EVENT, (e) => dispatchHermesHook(e.payload));
	return () => {
		binding.dispose();
		if (bindings.get(p.harnessId) === binding) bindings.delete(p.harnessId);
		if (bindings.size === 0 && unlisten) {
			const u = unlisten;
			unlisten = null;
			void u.then((f) => f());
		}
	};
}
