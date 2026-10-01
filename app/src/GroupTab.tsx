import { type MouseEvent as ReactMouseEvent, useMemo } from "react";
import { RoomNameInput, StatusDot } from "./components.tsx";
import { useRoomActivity } from "./harnessActivity.ts";
import { type StripSegment, groupDisplayName, groupRooms, segmentId } from "./roomGroups.ts";
import { type RoomDragWiring, useStableRooms } from "./roomStripShared.tsx";

/// A TOP-LEVEL group tab: same two-line shape as a room tab (so a
/// group reads as a tab, not a different kind of chrome) but its own
/// component — aggregate status dot (folded over every room the group
/// holds), the repo label, summed badge, room count on line 1;
/// "main · N worktrees" (or "main not open · N worktrees" when the
/// lead isn't open) on line 2. No close button — closing happens per
/// room, in the second row.
export const GroupTab = ({
	seg,
	active,
	onClick,
	dragWiring,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
}: {
	seg: Extract<StripSegment, { kind: "group" }>;
	active: boolean;
	onClick: () => void;
	dragWiring: RoomDragWiring;
	/** #241: renamable only when `seg.lead` is open — `RoomStrip` omits
	 *  `onStartRename` entirely for a placeholder-led group, which is
	 *  what keeps the "not renamable when main isn't open" rule here
	 *  without this component re-deriving it. */
	renaming?: boolean | undefined;
	onStartRename?: (() => void) | undefined;
	onRename?: ((name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
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
	// #328: read straight off `groupRooms(seg)`, not the (intentionally
	// stale-tolerant) `rooms` above — `useStableRooms` only re-derives on
	// an id/harnesses-reference change, so a bare `attention` flip on an
	// otherwise-unchanged member would go unseen through it.
	const hasAttention = groupRooms(seg).some((r) => r.attention);

	const segId = segmentId(seg);
	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = dragWiring;
	const isDragged = drag?.kind === "room" && drag.id === segId;
	const dropSide = dropTarget?.kind === "room" && dropTarget.id === segId ? dropTarget.side : null;
	const displayName = groupDisplayName(seg);

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
			onDoubleClick={(e: ReactMouseEvent<HTMLDivElement>) => {
				// #241: same reasoning as `RoomTab`'s own dblclick handler —
				// this root has pointer capture from `startDrag` above, so
				// `e.target` can't be trusted; hit-test the real point and
				// only start a rename if it's over this tab's `.name` span.
				// `onStartRename` is only wired when `seg.lead` is open.
				if (!onStartRename || renaming) return;
				const hit = document.elementFromPoint(e.clientX, e.clientY);
				const nameEl = hit instanceof Element ? hit.closest(".name") : null;
				if (!nameEl || !e.currentTarget.contains(nameEl)) return;
				onStartRename();
			}}
		>
			<div className="row-1">
				<StatusDot status={status} roomIds={rooms.map((r) => r.id)} aggName={displayName} />
				{renaming ? (
					<RoomNameInput
						initial={displayName}
						onCommit={(name) => {
							onRename?.(name);
							onRenameEnd?.();
						}}
						onCancel={() => onRenameEnd?.()}
					/>
				) : (
					<span className="name" title={displayName}>
						{displayName}
					</span>
				)}
				{badge > 0 && <span className="tab-badge">{badge}</span>}
				{hasAttention && <span className="tab-attention-dot" title="New room" />}
				<span className="sk-group-count">({roomCount})</span>
			</div>
			<div className="row-2">
				<span className="branch">
					{seg.lead ? "main" : "main not open"} · {worktreeCount} worktree
					{worktreeCount === 1 ? "" : "s"}
				</span>
			</div>
		</div>
	);
};
