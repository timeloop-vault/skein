import { type StripSegment, segmentId } from "./roomGroups.ts";
import { LiveRoomTab, type RenameTarget, type RoomDragWiring } from "./roomStripShared.tsx";
import "./RoomStrip.css";

/// The SECOND row: the active group's own rooms, main pinned first —
/// a real lead renders as a normal (undraggable) room tab, an unopen
/// one as a dimmed placeholder; then every member, draggable within
/// this row only (`resolveRowDrop`); then `+` to open New Room already
/// prefilled to this repository. App.tsx mounts this only when the
/// active room's segment IS a group (`segmentOfRoom`) — a repository
/// with a single open room has no second row and looks exactly like a
/// plain tab.
export const GroupRow = ({
	seg,
	activeRoomId,
	onSwitchRoom,
	onCloseRoom,
	onOpenPlaceholder,
	onNewRoom,
	dragWiring,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
}: {
	seg: Extract<StripSegment, { kind: "group" }>;
	activeRoomId: string | null;
	onSwitchRoom: (id: string) => void;
	onCloseRoom: (id: string) => void;
	/** Called with the group's key (for matching an archived main room)
	 *  and a real, un-normalized folder path (for prefilling New Room)
	 *  when the placeholder lead is clicked — App.tsx decides
	 *  reopen-archived vs. New Room. */
	onOpenPlaceholder: (key: string, folder: string) => void;
	onNewRoom: (folder: string) => void;
	dragWiring: RoomDragWiring;
	/** #241: same rename wiring as `RoomStrip` — covers the lead (a real
	 *  open main room, always `host: "tab"` here — the palette's rename
	 *  of an active main room lands on this second-row tab, not the
	 *  top-row `GroupTab`) and every member, never the dimmed placeholder. */
	renaming?: RenameTarget | null | undefined;
	onStartRename?: ((id: string, host: "group" | "tab") => void) | undefined;
	onRename?: ((id: string, name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
}) => {
	const segId = segmentId(seg);
	const lead = seg.lead;
	const folder = (lead ?? seg.members[0])?.repoRoot ?? seg.key;

	return (
		<div className="sk-grouprow" data-drag-strip="room">
			{lead ? (
				<LiveRoomTab
					key={lead.id}
					room={lead}
					active={lead.id === activeRoomId}
					onClick={() => onSwitchRoom(lead.id)}
					onClose={() => onCloseRoom(lead.id)}
					renaming={renaming?.roomId === lead.id && renaming.host === "tab"}
					onStartRename={() => onStartRename?.(lead.id, "tab")}
					onRename={(name) => onRename?.(lead.id, name)}
					onRenameEnd={onRenameEnd}
				/>
			) : (
				<div
					className="sk-tab sk-tab-placeholder"
					onClick={() => onOpenPlaceholder(seg.key, folder)}
				>
					<div className="row-1">
						<span className="tab-status-placeholder" aria-hidden="true" />
						<span className="name">{seg.label}</span>
					</div>
					<div className="row-2">
						<span className="branch">main not open</span>
					</div>
				</div>
			)}
			{seg.members.map((m) => (
				<LiveRoomTab
					key={m.id}
					room={m}
					active={m.id === activeRoomId}
					onClick={() => onSwitchRoom(m.id)}
					onClose={() => onCloseRoom(m.id)}
					dragWiring={dragWiring}
					dragSegId={segId}
					dragRole="member"
					renaming={renaming?.roomId === m.id && renaming.host === "tab"}
					onStartRename={() => onStartRename?.(m.id, "tab")}
					onRename={(name) => onRename?.(m.id, name)}
					onRenameEnd={onRenameEnd}
				/>
			))}
			<div className="sk-tab-newbtn" onClick={() => onNewRoom(folder)} title="New room">
				+
			</div>
		</div>
	);
};
