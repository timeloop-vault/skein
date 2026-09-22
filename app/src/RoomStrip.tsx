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

import { type PointerEvent as ReactPointerEvent, useMemo, useRef } from "react";
import { RoomTab, StatusDot } from "./components.tsx";
import { useRoomActivity } from "./harnessActivity.ts";
import { type StripSegment, groupRooms, segmentId } from "./roomGroups.ts";
import type { TabDragInfo } from "./tabDrag.ts";
import type { Room } from "./types.ts";
import type { TabDropTarget } from "./useTabDrag.ts";

/// `rooms`, but the SAME array reference across renders as long as its
/// content hasn't changed — membership (room ids, in order) and each
/// member's own `harnesses` array reference. Every call site below
/// builds `rooms` fresh each render, which would otherwise invalidate
/// `useMemo`/`useRoomActivity` downstream on every render regardless of
/// whether anything the tab actually shows moved.
function useStableRooms(rooms: readonly Room[]): readonly Room[] {
	const ref = useRef(rooms);
	const prev = ref.current;
	const unchanged =
		prev.length === rooms.length &&
		prev.every((r, i) => r.id === rooms[i]?.id && r.harnesses === rooms[i]?.harnesses);
	if (!unchanged) {
		ref.current = rooms;
	}
	return ref.current;
}

/** The slice of `useTabDrag`'s return value every draggable tab in the
 *  strip needs — passed through unchanged to each one. */
export interface RoomDragWiring {
	drag: TabDragInfo | null;
	dropTarget: TabDropTarget;
	startDrag: (e: ReactPointerEvent<HTMLDivElement>, info: TabDragInfo) => void;
	dragHandlers: {
		onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onPointerUp: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onPointerCancel: (e: ReactPointerEvent<HTMLDivElement>) => void;
		onLostPointerCapture: (e: ReactPointerEvent<HTMLDivElement>) => void;
	};
	suppressClick: () => boolean;
}

/// One room tab (plain top-level tab, a group's lead in the second
/// row, or a group member) with its OWN live status/badge — the same
/// per-room `useRoomActivity` fold every room tab has always used.
/// `dragWiring` omitted entirely renders a pinned tab with no drag
/// attributes at all: that's how a group's lead stays undraggable and
/// an invalid drop target (#76 point 6) without `resolveRowDrop` having
/// to refuse it at the DOM layer too.
const LiveRoomTab = ({
	room,
	active,
	onClick,
	onClose,
	dragWiring,
	dragSegId,
	dragRole,
}: {
	room: Room;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
	dragWiring?: RoomDragWiring;
	dragSegId?: string;
	dragRole?: "segment" | "member";
}) => {
	const stableRooms = useStableRooms([room]);
	const harnessRefs = useMemo(
		() =>
			stableRooms.flatMap((r) =>
				r.harnesses.map((h) => ({ id: h.id, pendingNotifications: h.pendingNotifications })),
			),
		[stableRooms],
	);
	const status = useRoomActivity(harnessRefs);
	const badge = room.harnesses.reduce((acc, h) => acc + (h.pendingNotifications ?? 0), 0);
	const derived: Room = { ...room, status, badge };

	if (!dragWiring) {
		return <RoomTab r={derived} active={active} onClick={onClick} onClose={onClose} />;
	}

	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = dragWiring;
	const isDragged = drag?.kind === "room" && drag.id === room.id;
	const dropSide =
		dropTarget?.kind === "room" && dropTarget.id === room.id ? dropTarget.side : null;
	const segId = dragSegId ?? room.id;
	const role = dragRole ?? "segment";

	return (
		<RoomTab
			r={derived}
			active={active}
			onClick={onClick}
			onClose={onClose}
			dragging={isDragged}
			dropSide={dropSide}
			dragKind="room"
			dragId={room.id}
			dragSegId={segId}
			dragRole={role}
			onPointerDown={(e) => startDrag(e, { kind: "room", id: room.id, segId, role })}
			onPointerMove={dragHandlers.onPointerMove}
			onPointerUp={dragHandlers.onPointerUp}
			onPointerCancel={dragHandlers.onPointerCancel}
			onLostPointerCapture={dragHandlers.onLostPointerCapture}
			suppressClick={suppressClick}
		/>
	);
};

/// A TOP-LEVEL group tab: same two-line shape as a room tab (so a
/// group reads as a tab, not a different kind of chrome) but its own
/// component — aggregate status dot (folded over every room the group
/// holds), the repo label, summed badge, room count on line 1;
/// "main · N worktrees" (or "main not open · N worktrees" when the
/// lead isn't open) on line 2. No close button — closing happens per
/// room, in the second row.
const GroupTab = ({
	seg,
	active,
	onClick,
	dragWiring,
}: {
	seg: Extract<StripSegment, { kind: "group" }>;
	active: boolean;
	onClick: () => void;
	dragWiring: RoomDragWiring;
}) => {
	const rooms = useStableRooms(groupRooms(seg));
	const harnessRefs = useMemo(
		() =>
			rooms.flatMap((r) =>
				r.harnesses.map((h) => ({ id: h.id, pendingNotifications: h.pendingNotifications })),
			),
		[rooms],
	);
	const status = useRoomActivity(harnessRefs);
	const badge = rooms.reduce(
		(acc, r) => acc + r.harnesses.reduce((a, h) => a + (h.pendingNotifications ?? 0), 0),
		0,
	);
	const roomCount = rooms.length;
	const worktreeCount = seg.members.length;

	const segId = segmentId(seg);
	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = dragWiring;
	const isDragged = drag?.kind === "room" && drag.id === segId;
	const dropSide = dropTarget?.kind === "room" && dropTarget.id === segId ? dropTarget.side : null;

	return (
		<div
			className={`sk-tab sk-group-tab ${active ? "active" : ""} ${isDragged ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
			data-drag-kind="room"
			data-drag-id={segId}
			data-drag-seg={segId}
			data-drag-role="segment"
			onPointerDown={(e) => startDrag(e, { kind: "room", id: segId, segId, role: "segment" })}
			onPointerMove={dragHandlers.onPointerMove}
			onPointerUp={dragHandlers.onPointerUp}
			onPointerCancel={dragHandlers.onPointerCancel}
			onLostPointerCapture={dragHandlers.onLostPointerCapture}
			onClick={() => {
				// #271: see RoomTab's onClick — same swallow-the-post-drop-click.
				if (suppressClick()) return;
				onClick();
			}}
		>
			<div className="row-1">
				<StatusDot status={status} />
				<span className="name" title={seg.label}>
					{seg.label}
				</span>
				{badge > 0 && <span className="tab-badge">{badge}</span>}
				<span className="sk-group-count">({roomCount})</span>
			</div>
			<div className="row-2">
				<span>
					{seg.lead ? "main" : "main not open"} · {worktreeCount} worktree
					{worktreeCount === 1 ? "" : "s"}
				</span>
			</div>
		</div>
	);
};

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
}: {
	segments: readonly StripSegment[];
	activeRoomId: string | null;
	onSelectSegment: (seg: StripSegment) => void;
	onCloseRoom: (id: string) => void;
	dragWiring: RoomDragWiring;
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
					/>
				);
			}
			const active =
				seg.lead?.id === activeRoomId || seg.members.some((m) => m.id === activeRoomId);
			return (
				<GroupTab
					key={`g:${seg.key}`}
					seg={seg}
					active={active}
					onClick={() => onSelectSegment(seg)}
					dragWiring={dragWiring}
				/>
			);
		})}
	</>
);

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
						<span>main not open</span>
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
				/>
			))}
			<div className="sk-tab-newbtn" onClick={() => onNewRoom(folder)} title="New room">
				+
			</div>
		</div>
	);
};
