// The two Claude command-hook events useHarnessNotifications listens to
// (#459 split; no behaviour change): `harness-permission` (#86) and
// `harness-session-start` (#273/#116). Both are global, room-agnostic
// Tauri events whose payload names the harness.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MutableRefObject } from "react";
import { useEffect } from "react";
import { outstandingWork } from "./deferral.ts";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { followedSession } from "./sessionTracking.ts";
import { shellClaim, shouldReplaceClaim } from "./shellClaim.ts";
import type { Room } from "./types.ts";

export function useHarnessHookEvents(
	roomsRef: MutableRefObject<Room[]>,
	replaceHarnessSessionId: (targetRoomId: string, harnessId: string, sessionId: string) => void,
) {
	// #86: Claude's PermissionRequest hook fires this global event the
	// moment a permission dialog is on screen — the Rust side owns the
	// hook wiring, this is just the frontend half of the contract.
	// Room-agnostic by design: the harness id alone is enough to route
	// it, and `setPermissionFromAdapter` is already a no-op for an id
	// Skein doesn't have a record for (a stale event racing a closed
	// harness).
	useEffect(() => {
		const un = listen<{
			roomId: string;
			harnessId: string;
			toolName: string | null;
			agentType: string | null;
			agentId: string | null;
			sessionId?: string | null;
		}>("skein://harness-permission", (event) => {
			// #318: in a post-exit shell, only the claimed claude's pings count.
			if (!shellClaim.acceptsPermission(event.payload.harnessId, event.payload.sessionId)) {
				console.debug("[skein] dropped permission ping from unclaimed shell session");
				return;
			}
			harnessActivity.setPermissionFromAdapter(
				event.payload.harnessId,
				TRANSITION_SOURCE.L2c1ClaudePermission,
				event.payload.toolName,
				event.payload.agentType,
				event.payload.agentId,
			);
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);

	// #318: the shell-mode half of the session-start listener.
	const followShellStart = async (
		roomId: string,
		harnessId: string,
		current: string | undefined,
		payload: { sessionId: string | null; source: string | null },
	) => {
		const act = harnessActivity.get(harnessId);
		const claimedBusy =
			act?.phase === "running" || act?.phase === "permission" || outstandingWork(harnessId) > 0;
		let d = shellClaim.onSessionStart(harnessId, current, payload, claimedBusy);
		if (d.kind === "probe-then-replace") {
			// A different-id `startup` while claimed: a nested child (claimed
			// claude busy, transcript real), a phantom's successor, or a new
			// claude after a lost SessionEnd (claimed claude idle).
			const claimedId = d.claimedId;
			const exists = d.claimedBusy
				? await invoke<boolean>("claude_session_exists", { id: claimedId }).catch(() => true)
				: false;
			// The claim or stored id may have moved during the probe.
			const stored = roomsRef.current
				.find((r) => r.id === roomId)
				?.harnesses.find((x) => x.id === harnessId)?.sessionId;
			if (shellClaim.claimedId(harnessId) !== claimedId || stored !== current) return;
			if (!shouldReplaceClaim(exists, d.claimedBusy)) {
				console.debug("[skein] ignored startup while claimed (nested child)");
				return;
			}
			shellClaim.replaceClaim(harnessId, d.sessionId);
			d = {
				kind: "follow",
				sessionId: d.sessionId,
				source: "startup",
				claim: true,
				repoint: d.sessionId !== current,
			};
		}
		if (d.kind === "ignore") {
			console.debug("[skein] ignored session-start in shell mode");
			return;
		}
		harnessActivity.noteLaunchSignal(harnessId);
		if (d.repoint) {
			replaceHarnessSessionId(roomId, harnessId, d.sessionId);
			// `startup` has no phase source of its own; it reads like a resume.
			harnessActivity.sessionSwitched(
				harnessId,
				d.source === "clear" || d.source === "fork" ? d.source : "resume",
			);
		} else {
			// Same id: the tail is already on this file; just drop the one-shot flag.
			shellClaim.consumeFreshProcess(harnessId);
		}
	};

	// #273: Claude's `SessionStart` command hook fires this global event
	// the moment its CLI has (re)started — including before any
	// transcript exists, which is exactly the gap that used to leave a
	// freshly spawned harness stuck in `spawning` forever. Room-agnostic
	// like the permission listener above; `noteLaunchSignal` is already
	// a no-op for an id Skein doesn't have a record for.
	//
	// #116: the same hook is the ONLY signal Skein gets that a
	// mid-session `/clear`, in-tool `/resume` or a fork (`/branch`,
	// `/fork`, `--fork-session`, desktop rewind) moved a harness onto a
	// different conversation — Claude's JSONL gives no other sign.
	// `followedSession` (sessionTracking.ts) filters to `source ===
	// "clear"`, `"resume"` or `"fork"` reporting a genuinely different
	// id; when it does, the harness's stored sessionId is overwritten
	// (`replaceHarnessSessionId`, first-writer-wins would never adopt it)
	// so a future resume/reopen picks up the new conversation, and
	// `harnessActivity.sessionSwitched` forgets the old session's
	// subagents/delegation state. Per-pane re-pointing of the live JSONL
	// tail itself happens in LiveTerminal, keyed off the `sessionId`
	// prop — this listener only owns the persisted record. Attribution
	// for which pane the hook fired in comes from `SKEIN_HARNESS_ID` via
	// the `X-Skein-Harness` header, not from anything computed here.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const un = listen<{
			roomId: string;
			harnessId: string;
			sessionId: string | null;
			source: string | null;
		}>("skein://harness-session-start", (event) => {
			const { roomId, harnessId } = event.payload;
			const room = roomsRef.current.find((r) => r.id === roomId);
			const h = room?.harnesses.find((x) => x.id === harnessId);
			// #318: a post-exit shell's claude is decided by the claim gate;
			// an ignored event (a nested child) must not touch the phase.
			if (h?.kind === "claude" && shellClaim.isShell(harnessId)) {
				void followShellStart(roomId, harnessId, h.sessionId, event.payload);
				return;
			}
			harnessActivity.noteLaunchSignal(harnessId);
			if (h?.kind !== "claude") return;
			const next = followedSession(h.sessionId, event.payload);
			if (next !== null) {
				replaceHarnessSessionId(roomId, harnessId, next.sessionId);
				harnessActivity.sessionSwitched(harnessId, next.source);
			}
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);

	// #318: SessionEnd releases the shell claim so the next claude typed in
	// the same shell can claim. No phase change.
	useEffect(() => {
		const un = listen<{
			roomId: string;
			harnessId: string;
			sessionId: string | null;
			reason: string | null;
		}>("skein://harness-session-end", (event) => {
			shellClaim.onSessionEnd(event.payload.harnessId, event.payload);
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);
}
