// A `design` harness mounted in a room: adapts the room-wide change
// handlers to DesignBody's per-harness props (#433, #528).

import { DesignBody } from "./DesignBody.tsx";
import { type DesignDevice, normalizeDevice } from "./designDevice.ts";
import type { Harness, Room } from "./types.ts";

export type DeviceChangeHandler = (
	roomId: string,
	harnessId: string,
	device: DesignDevice | undefined,
) => void;

interface Props {
	room: Room;
	harness: Harness;
	visible: boolean;
	onEntryChange: (roomId: string, harnessId: string, entry: string) => void;
	onDeviceChange: DeviceChangeHandler;
}

export const DesignHarnessBody = ({
	room,
	harness: h,
	visible,
	onEntryChange,
	onDeviceChange,
}: Props) => (
	<DesignBody
		harnessId={h.id}
		roomId={room.id}
		cwd={h.cwd ?? room.cwd ?? ""}
		visible={visible}
		entry={h.designEntry}
		onEntryChange={(entry) => onEntryChange(room.id, h.id, entry)}
		device={normalizeDevice(h.designDevice)}
		onDeviceChange={(d) => onDeviceChange(room.id, h.id, d)}
	/>
);
