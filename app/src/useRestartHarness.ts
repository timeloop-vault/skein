import type { MutableRefObject } from "react";
import type { GateResult } from "./harnessInputGate.ts";
import type { Room } from "./types.ts";
import type { useHarnessNotifications } from "./useHarnessNotifications.ts";

type PushToast = ReturnType<typeof useHarnessNotifications>["pushToast"];

// #490: palette entry point — a refusal (the gate re-checks live) surfaces
// as an info toast, since the palette has no disabled state.
export function useRestartHarness(
	roomsRef: MutableRefObject<Room[]>,
	restartHarness: (roomId: string, harnessId: string) => Promise<GateResult>,
	pushToast: PushToast,
) {
	return (roomId: string, harnessId: string) => {
		void restartHarness(roomId, harnessId).then((result) => {
			if (result.ok) return;
			const owningRoom = roomsRef.current.find((r) => r.id === roomId);
			const harness = owningRoom?.harnesses.find((h) => h.id === harnessId);
			if (!owningRoom || !harness) return;
			pushToast({
				id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
				roomId,
				harnessId,
				kind: harness.kind,
				roomName: owningRoom.name,
				harnessName: harness.name,
				state: "info",
				message: `Can't restart: ${result.reason}`,
			});
		});
	};
}
