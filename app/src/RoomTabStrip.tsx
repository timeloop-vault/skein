import type { ComponentProps } from "react";
import { GroupRow, type RenameTarget, RoomStrip } from "./RoomStrip.tsx";
import type { StripSegment } from "./roomGroups.ts";
import { hints } from "./shortcuts.ts";
import { shownActiveId } from "./tempView.ts";
import "./RoomStrip.css";

type StripProps = ComponentProps<typeof RoomStrip>;

// The two-level room strip: the top row (one tab per segment, plus the
// `+` button) and, when the active room is IN a group, the second row.
export const RoomTabStrip = ({
	stripSegments,
	activeSegment,
	activeRoomId,
	onSelectSegment,
	closeRoom,
	openNewRoom,
	openNewRoomAt,
	openGroupPlaceholder,
	switchRoom,
	dragWiring,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
	controlCenterOpen,
	onToggleControlCenter,
}: {
	stripSegments: StripSegment[];
	activeSegment: StripSegment | undefined;
	activeRoomId: string;
	onSelectSegment: StripProps["onSelectSegment"];
	closeRoom: (id: string) => Promise<void> | void;
	openNewRoom: () => Promise<void> | void;
	openNewRoomAt: ComponentProps<typeof GroupRow>["onNewRoom"];
	openGroupPlaceholder: ComponentProps<typeof GroupRow>["onOpenPlaceholder"];
	switchRoom: ComponentProps<typeof GroupRow>["onSwitchRoom"];
	dragWiring: StripProps["dragWiring"];
	renaming: RenameTarget | null;
	onStartRename: StripProps["onStartRename"];
	onRename: StripProps["onRename"];
	onRenameEnd: StripProps["onRenameEnd"];
	controlCenterOpen: boolean;
	onToggleControlCenter: () => void;
}) => {
	// #560: no room tab reads as active while the Control Center owns the area.
	const shown = shownActiveId(activeRoomId, controlCenterOpen);
	return (
		<>
			{/* #271: data-drag-strip lets useTabDrag's hitTest resolve a drop
		    over blank strip space or the `+` button to an end-of-strip
		    gap, instead of finding nothing draggable there. */}
			<div className="sk-tabstrip" data-drag-strip="room">
				<RoomStrip
					segments={stripSegments}
					activeRoomId={shown}
					onSelectSegment={onSelectSegment}
					onCloseRoom={(id) => void closeRoom(id)}
					dragWiring={dragWiring}
					renaming={renaming}
					onStartRename={onStartRename}
					onRename={onRename}
					onRenameEnd={onRenameEnd}
				/>
				<div className="sk-tab-newbtn" onClick={() => void openNewRoom()} title="New room">
					+
				</div>
				<button
					type="button"
					className="sk-tab-newbtn sk-tab-ccbtn"
					aria-pressed={controlCenterOpen}
					title={`Control Center (${hints.controlCenter})`}
					onClick={onToggleControlCenter}
				>
					▦
				</button>
			</div>
			{/* #76: the second row — the active group's own rooms, main
		    pinned first — only when the active room is IN a group. A
		    repository with a single open room stays a plain top-level
		    tab and never grows this row. */}
			{activeSegment?.kind === "group" && (
				<GroupRow
					seg={activeSegment}
					activeRoomId={shown}
					onSwitchRoom={switchRoom}
					onCloseRoom={(id) => void closeRoom(id)}
					onOpenPlaceholder={openGroupPlaceholder}
					onNewRoom={openNewRoomAt}
					dragWiring={dragWiring}
					renaming={renaming}
					onStartRename={onStartRename}
					onRename={onRename}
					onRenameEnd={onRenameEnd}
				/>
			)}
		</>
	);
};
