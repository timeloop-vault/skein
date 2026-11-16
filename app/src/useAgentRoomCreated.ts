// Listens for the daemon's `skein://agent-room-created` poke (#734) and
// pushes the "created" toast.

import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import {
	AGENT_ROOM_CREATED_EVENT,
	agentRoomCreatedToast,
	parseAgentRoomCreated,
} from "./agentRoomCreated.ts";
import type { NewToast } from "./toastStack.ts";

export function useAgentRoomCreated(pushToast: (entry: NewToast) => void) {
	const pushRef = useRef(pushToast);
	pushRef.current = pushToast;
	useEffect(() => {
		let disposed = false;
		let unlisten: (() => void) | undefined;
		void listen<unknown>(AGENT_ROOM_CREATED_EVENT, (e) => {
			const p = parseAgentRoomCreated(e.payload);
			if (!p) return;
			const id = `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
			pushRef.current(agentRoomCreatedToast(p, id));
		}).then((u) => {
			if (disposed) u();
			else unlisten = u;
		});
		return () => {
			disposed = true;
			unlisten?.();
		};
	}, []);
}
