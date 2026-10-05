// The right pane's Design tab body (#551): every docked design harness
// of the room stays mounted (display:none unless shown), so a switch or a
// tab change never reloads a preview. A picker appears with two or more.

import { DesignHarnessBody, type DeviceChangeHandler } from "./DesignHarnessBody.tsx";
import type { Harness, Room } from "./types.ts";

export interface RightPaneDesignProps {
	room: Room;
	/** The room's docked design harnesses, in room order (never empty here). */
	docked: Harness[];
	/** The harness the tab shows. */
	shown: Harness | undefined;
	/** Pane visible and the Design tab selected. */
	visible: boolean;
	onPick: (harnessId: string) => void;
	onEntryChange: (roomId: string, harnessId: string, entry: string) => void;
	onDeviceChange: DeviceChangeHandler;
	/** Undock a harness: it returns to the harness column (#551). */
	onUndock: (harnessId: string) => void;
}

export const RightPaneDesign = ({
	room,
	docked,
	shown,
	visible,
	onPick,
	onEntryChange,
	onDeviceChange,
	onUndock,
}: RightPaneDesignProps) => (
	<>
		{docked.length > 1 && (
			<div className="sk-rp-design-picker">
				<select
					aria-label="Docked design harness"
					value={shown?.id ?? ""}
					onChange={(e) => onPick(e.target.value)}
				>
					{docked.map((h) => (
						<option key={h.id} value={h.id}>
							{h.name}
						</option>
					))}
				</select>
			</div>
		)}
		{docked.map((h) => (
			<div
				key={h.id}
				className="sk-rp-design-body"
				style={{ display: h.id === shown?.id ? "flex" : "none" }}
			>
				<DesignHarnessBody
					room={room}
					harness={h}
					visible={visible && h.id === shown?.id}
					onEntryChange={onEntryChange}
					onDeviceChange={onDeviceChange}
					dock={{ docked: true, onToggle: () => onUndock(h.id) }}
				/>
			</div>
		))}
	</>
);
