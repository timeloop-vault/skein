import type { MutableRefObject } from "react";
import { envProfileState, setActiveEnvProfile } from "./envProfileStore.ts";
import { spawnedProfile } from "./spawnedEnvProfile.ts";
import { countStaleHarnesses, switchToastMessage } from "./staleProfiles.ts";
import type { Room } from "./types.ts";
import type { useHarnessNotifications } from "./useHarnessNotifications.ts";

type PushToast = ReturnType<typeof useHarnessNotifications>["pushToast"];

// #615: the one way the status-bar chip and the palette switch the global
// profile. Saves, then says what it does NOT do: running harnesses keep
// their env until restarted.
export function useEnvProfileSwitch(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomId: string,
	pushToast: PushToast,
) {
	return (name: string | null) => {
		void setActiveEnvProfile(name)
			.then(() => {
				const { names, active, loaded } = envProfileState();
				if (!loaded) return;
				const rooms = roomsRef.current.filter((r) => !r.archived);
				const owner = rooms.find((r) => r.id === activeRoomId);
				const harness = owner?.harnesses.find((h) => h.id === owner.activeHarnessId);
				if (!owner || !harness) return;
				const stale = countStaleHarnesses(rooms, active, names, spawnedProfile);
				pushToast({
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owner.id,
					harnessId: harness.id,
					kind: harness.kind,
					roomName: owner.name,
					harnessName: harness.name,
					state: "info",
					message: switchToastMessage(active, stale),
				});
			})
			.catch((err: unknown) => {
				// The store is unchanged, so the chip still shows the truth; say why.
				const owner = roomsRef.current.find((r) => r.id === activeRoomId);
				const harness = owner?.harnesses.find((h) => h.id === owner.activeHarnessId);
				if (!owner || !harness) return;
				pushToast({
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owner.id,
					harnessId: harness.id,
					kind: harness.kind,
					roomName: owner.name,
					harnessName: harness.name,
					// "info" is the variant that renders `message`; "error" would print the bare word.
					state: "info",
					message: `Couldn't switch env profile: ${err instanceof Error ? err.message : String(err)}`,
				});
			});
	};
}
