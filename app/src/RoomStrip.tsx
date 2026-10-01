// The room tab strip (#76): a two-level strip, not the rejected
// single-row-with-borders design. `RoomStrip` renders the TOP row —
// one tab per repository (a `plain` segment renders as today's room
// tab, unchanged; a `group` segment renders as a distinct `GroupTab`,
// aggregate status/badge/count, no close button). `GroupRow` renders
// the SECOND row — the active group's own rooms, main pinned first —
// and App.tsx mounts it only when the active room's segment is a
// group (`segmentOfRoom`). App.tsx owns the segment data (`buildStrip`),
// `lastUsedByGroup` and the keyboard/reorder plumbing; this module only
// renders it.
//
// Drag-to-reorder (#271) stays group-aware (#76): every room tab
// carries `dragKind="room"` / `dragId` PLUS `dragSegId`/`dragRole` —
// which strip segment (plain room, group, or group member) the tab's
// drag belongs to, and whether the tab IS that segment or only a
// non-lead MEMBER of one. Top-row tabs are always `role="segment"`;
// second-row members are `role="member"`; a group's lead (real or
// placeholder) is pinned — it renders with no drag wiring at all, so
// it can never be dragged and (per `resolveRowDrop`) is never a valid
// drop target either. App.tsx's `reorderRoom` resolves the actual move
// via `resolveTopDrop` (top row) or `resolveRowDrop` (second row),
// picking between them by re-deriving fresh from current `rooms`
// whether the dragged id is a non-lead group member.

import { GroupTab } from "./GroupTab.tsx";
import { type StripSegment, segmentId } from "./roomGroups.ts";
import { LiveRoomTab, type RenameTarget, type RoomDragWiring } from "./roomStripShared.tsx";

export { GroupRow } from "./GroupRow.tsx";
export { GroupTab } from "./GroupTab.tsx";
export type { RenameTarget, RoomDragWiring } from "./roomStripShared.tsx";

/// The top row: one tab per top-level segment — a plain room tab
/// unchanged, or a `GroupTab` for a repository with two or more open
/// rooms (or a worktree-only bucket). Clicking either calls
/// `onSelectSegment`, which App.tsx resolves via `topLevelTarget`.
export const RoomStrip = ({
	segments,
	activeRoomId,
	onSelectSegment,
	onCloseRoom,
	dragWiring,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
}: {
	segments: readonly StripSegment[];
	activeRoomId: string | null;
	onSelectSegment: (seg: StripSegment) => void;
	onCloseRoom: (id: string) => void;
	dragWiring: RoomDragWiring;
	/** #241: which room (if any) is mid-rename, and which tab hosts the
	 *  input (see `RenameTarget`) — plain top-level tabs are always
	 *  `"tab"`; a `GroupTab` only renames when its `host` is `"group"`
	 *  AND its own main room is open. */
	renaming?: RenameTarget | null | undefined;
	onStartRename?: ((id: string, host: "group" | "tab") => void) | undefined;
	onRename?: ((id: string, name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
}) => (
	<>
		{segments.map((seg) => {
			if (seg.kind === "plain") {
				return (
					<LiveRoomTab
						key={seg.room.id}
						room={seg.room}
						active={seg.room.id === activeRoomId}
						onClick={() => onSelectSegment(seg)}
						onClose={() => onCloseRoom(seg.room.id)}
						dragWiring={dragWiring}
						dragSegId={segmentId(seg)}
						dragRole="segment"
						renaming={renaming?.roomId === seg.room.id && renaming.host === "tab"}
						onStartRename={() => onStartRename?.(seg.room.id, "tab")}
						onRename={(name) => onRename?.(seg.room.id, name)}
						onRenameEnd={onRenameEnd}
					/>
				);
			}
			const active =
				seg.lead?.id === activeRoomId || seg.members.some((m) => m.id === activeRoomId);
			const leadId = seg.lead?.id;
			return (
				<GroupTab
					key={`g:${seg.key}`}
					seg={seg}
					active={active}
					onClick={() => onSelectSegment(seg)}
					dragWiring={dragWiring}
					renaming={
						leadId !== undefined && renaming?.roomId === leadId && renaming.host === "group"
					}
					onStartRename={leadId !== undefined ? () => onStartRename?.(leadId, "group") : undefined}
					onRename={leadId !== undefined ? (name: string) => onRename?.(leadId, name) : undefined}
					onRenameEnd={onRenameEnd}
				/>
			);
		})}
	</>
);
