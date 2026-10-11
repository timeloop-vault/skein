import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { overrideToastMessage } from "./envProfileMenu.ts";
import { envProfileState } from "./envProfileStore.ts";
import { spawnedProfile } from "./spawnedEnvProfile.ts";
import type { Room } from "./types.ts";
import type { useHarnessNotifications } from "./useHarnessNotifications.ts";

type PushToast = ReturnType<typeof useHarnessNotifications>["pushToast"];

// #615: the Actions menu's per-harness env profile. Persisted with the rooms
// blob and read at spawn time only, so there is no spawnGen bump — a running
// harness keeps its env until restarted, and the toast says so. `undefined`
// removes the key (follow the global profile).
export function useHarnessEnvProfile(
	roomsRef: MutableRefObject<Room[]>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	pushToast: PushToast,
) {
	return (roomId: string, harnessId: string, name: string | undefined) => {
		setRooms((prev) =>
			prev.map((r) =>
				r.id === roomId
					? {
							...r,
							harnesses: r.harnesses.map((h) => {
								if (h.id !== harnessId) return h;
								const { envProfile: _old, ...rest } = h;
								return name === undefined ? rest : { ...rest, envProfile: name };
							}),
						}
					: r,
			),
		);
		const room = roomsRef.current.find((r) => r.id === roomId);
		const harness = room?.harnesses.find((h) => h.id === harnessId);
		if (!room || !harness) return;
		const { names, active, loaded } = envProfileState();
		if (!loaded) return;
		const message = overrideToastMessage(
			harness.name,
			spawnedProfile(harnessId),
			name,
			active,
			names,
		);
		if (message === null) return;
		pushToast({
			id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
			roomId,
			harnessId,
			kind: harness.kind,
			roomName: room.name,
			harnessName: harness.name,
			state: "info",
			message,
		});
	};
}
