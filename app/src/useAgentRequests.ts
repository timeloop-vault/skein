// The frontend half of the agent request round trip: the listener for
// #328's `skein://agent-request`. Mounted once from App.tsx, alongside
// `useMailDisplay` (#329), which is the other hook driven entirely off
// Tauri events rather than `rooms` props.
//
// Since #734 the room and harness verbs (`create_room`, `close_room`,
// `open_harness`, `close_harness`) run in the daemon, and clients learn of
// their effects through `skein://rooms-changed`. What is left here is the
// design pane's requests (`design.*`), which only the webview can answer.
//
// Every request MUST be answered exactly once, `ok` or `error` — the
// Rust side is a real HTTP handler blocked on the answer (`state.rs`'s
// `await_response`), and an unanswered request just times out rather
// than hanging forever, but a silently-swallowed one would leave the
// calling agent's tool call erroring for the wrong reason. The `kind`
// dispatch below and its `catch` both funnel into `complete`, so a
// thrown error still answers rather than leaking.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MutableRefObject } from "react";
import { useEffect, useRef } from "react";
import { buildFilesDirtyAnswer, isFilesKind } from "./agentRequestsFiles.ts";
import { handleDesignRequest, isDesignKind } from "./designAgentRequests.ts";
import type { DesignDevice } from "./designDevice.ts";
import { filesRegistry } from "./filesRegistry.ts";
import type { Room } from "./types.ts";

interface AgentRequestPayload {
	id: string;
	kind: string;
	args: unknown;
}

export function useAgentRequests(
	// Full room list (including archived): the design requests look their
	// room up by id.
	roomsRef: MutableRefObject<Room[]>,
	// #512: `useHarnessActions.setHarnessDesignEntry` — the toolbar picker's
	// own persist path, used by `design.open_entry`.
	setDesignEntry: (roomId: string, harnessId: string, entry: string) => void,
	// #548: `useHarnessActions.setHarnessDesignDevice`, for `design.set_device`.
	setDesignDevice: (roomId: string, harnessId: string, device: DesignDevice | undefined) => void,
	// #549: `reveal` on show_element / invoke_element: the room in front and
	// `useHarnessActions.switchHarnessInRoom`; never switches rooms.
	activeRoomIdRef: MutableRefObject<string | null>,
	switchHarnessInRoom: (roomId: string, harnessId: string) => void,
	// #551: a docked design harness is revealed in the right pane instead.
	isDocked: (harnessId: string) => boolean,
	showDocked: (roomId: string, harnessId: string) => void,
	// #552: `show_design_pane` may switch rooms (the agent's MCP permission is
	// the gate): `useRoomsStore.switchRoom`, the same call a notification click makes.
	switchRoom: (roomId: string) => void,
): void {
	// Refs so the listener (mounted once, below) always reads the latest
	// values without re-subscribing on every settings/memory change —
	// same shape `useHarnessNotifications.ts`'s `notifyBadgeRef` and
	// friends use.
	const setDesignEntryRef = useRef(setDesignEntry);
	setDesignEntryRef.current = setDesignEntry;
	const setDesignDeviceRef = useRef(setDesignDevice);
	setDesignDeviceRef.current = setDesignDevice;
	const switchHarnessRef = useRef(switchHarnessInRoom);
	switchHarnessRef.current = switchHarnessInRoom;
	const isDockedRef = useRef(isDocked);
	isDockedRef.current = isDocked;
	const showDockedRef = useRef(showDocked);
	showDockedRef.current = showDocked;
	const switchRoomRef = useRef(switchRoom);
	switchRoomRef.current = switchRoom;

	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a parameter.
	useEffect(() => {
		const complete = (id: string, ok?: unknown, error?: string): Promise<void> =>
			invoke<void>("agent_request_complete", { id, ok: ok ?? null, error: error ?? null }).catch(
				(err: unknown) => {
					console.warn(`[skein] agent_request_complete failed for ${id}:`, err);
				},
			);

		const unlistenPromise = listen<AgentRequestPayload>("skein://agent-request", (event) => {
			const { id, kind, args } = event.payload;
			void (async () => {
				try {
					if (isDesignKind(kind)) {
						await handleDesignRequest(
							kind,
							id,
							args,
							roomsRef.current,
							setDesignEntryRef.current,
							setDesignDeviceRef.current,
							complete,
							{
								activeRoomId: () => activeRoomIdRef.current,
								switchRoom: (r) => switchRoomRef.current(r),
								switchHarness: (r, h) => switchHarnessRef.current(r, h),
								isDocked: (h) => isDockedRef.current(h),
								showDocked: (r, h) => showDockedRef.current(r, h),
							},
						);
					} else if (isFilesKind(kind)) {
						const r = buildFilesDirtyAnswer(args, (h) => filesRegistry.dirtyNames(h));
						await (r.ok ? complete(id, r.value) : complete(id, undefined, r.error));
					} else {
						await complete(id, undefined, `unknown agent request kind "${kind}"`);
					}
				} catch (err: unknown) {
					const msg = err instanceof Error ? err.message : String(err);
					await complete(id, undefined, msg);
				}
			})();
		});
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);
}
