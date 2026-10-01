// The two Claude command-hook events useHarnessNotifications listens to
// (#459 split; no behaviour change): `harness-permission` (#86) and
// `harness-session-start` (#273/#116). Both are global, room-agnostic
// Tauri events whose payload names the harness.

import { listen } from "@tauri-apps/api/event";
import type { MutableRefObject } from "react";
import { useEffect } from "react";
import { TRANSITION_SOURCE, harnessActivity } from "./harnessActivity.ts";
import { followedSession } from "./sessionTracking.ts";
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
		}>("skein://harness-permission", (event) => {
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
			harnessActivity.noteLaunchSignal(event.payload.harnessId);
			const room = roomsRef.current.find((r) => r.id === event.payload.roomId);
			const h = room?.harnesses.find((x) => x.id === event.payload.harnessId);
			if (h?.kind !== "claude") return;
			const next = followedSession(h.sessionId, event.payload);
			if (next !== null) {
				replaceHarnessSessionId(event.payload.roomId, event.payload.harnessId, next.sessionId);
				harnessActivity.sessionSwitched(event.payload.harnessId, next.source);
			}
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);
}
