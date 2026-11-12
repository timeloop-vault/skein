import { type Dispatch, type MutableRefObject, type SetStateAction, useEffect } from "react";
import { setArgvFailureSink } from "./argvFailure.ts";
import { useEnvProfiles } from "./envProfileStore.ts";
import type { Room } from "./types.ts";
import { useEnvProfileSwitch } from "./useEnvProfileSwitch.ts";
import { useHarnessEnvProfile } from "./useHarnessEnvProfile.ts";
import type { useHarnessNotifications } from "./useHarnessNotifications.ts";
import { useReattachTelemetry } from "./useReattachTelemetry.ts";
import { useRestartHarness } from "./useRestartHarness.ts";

type PushToast = ReturnType<typeof useHarnessNotifications>["pushToast"];

// App's per-harness menu/palette callbacks: #410 reattach telemetry, #490
// restart, and #615 env profiles (the status-bar chip + palette switch the
// global profile; the Actions menu sets a per-harness override).
export function useAppHarnessMenu(
	roomsRef: MutableRefObject<Room[]>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	activeRoomId: string,
	restartHarness: Parameters<typeof useRestartHarness>[1],
	pushToast: PushToast,
) {
	// #730: actions with no result to return (add harness, new room, reopen,
	// repoint) toast through this when the daemon refuses to build an argv.
	useEffect(() => {
		setArgvFailureSink(pushToast);
		return () => setArgvFailureSink(null);
	}, [pushToast]);
	const onReattachTelemetry = useReattachTelemetry(roomsRef, pushToast);
	const onRestartHarness = useRestartHarness(roomsRef, restartHarness, pushToast);
	const envProfiles = useEnvProfiles();
	const onSwitchEnvProfile = useEnvProfileSwitch(roomsRef, activeRoomId, pushToast);
	const onEnvProfileChange = useHarnessEnvProfile(roomsRef, setRooms, pushToast);
	return {
		onReattachTelemetry,
		onRestartHarness,
		envProfiles,
		onSwitchEnvProfile,
		onEnvProfileChange,
	};
}
