import type { MouseEvent as ReactMouseEvent } from "react";
import type { DragProps } from "./dragProps.ts";
import { HChip, StatusDot } from "./HChip.tsx";
import { RoomNameInput } from "./RoomNameInput.tsx";
import { useTodoMenu } from "./todos/TodoMenuProvider.tsx";
import type { Room } from "./types.ts";
import "./RoomStrip.css";

export const RoomTab = ({
	r,
	active,
	onClick,
	onClose,
	renaming,
	onStartRename,
	onRename,
	onRenameEnd,
	dragging,
	dropSide,
	dragKind,
	dragId,
	dragRoomId,
	dragSegId,
	dragRole,
	onPointerDown,
	onPointerMove,
	onPointerUp,
	onPointerCancel,
	onLostPointerCapture,
	suppressClick,
}: {
	r: Room;
	active: boolean;
	onClick: () => void;
	onClose: () => void;
	/** #241: inline rename. All optional — a caller that doesn't wire
	 *  these (the group lead placeholder, tests) just gets the static
	 *  name span with no way to enter rename mode. */
	renaming?: boolean | undefined;
	onStartRename?: (() => void) | undefined;
	onRename?: ((name: string) => void) | undefined;
	onRenameEnd?: (() => void) | undefined;
} & DragProps) => {
	const openTodoMenu = useTodoMenu();
	return (
		<div
			className={`sk-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
			// #335: right-click → add this room (+ its active harness) as a todo.
			onContextMenu={(e) => {
				// Leave the native menu in the inline rename field (cut/copy/paste).
				if (
					renaming ||
					e.target instanceof HTMLInputElement ||
					e.target instanceof HTMLTextAreaElement
				)
					return;
				openTodoMenu(e, r.id, r.activeHarnessId || undefined);
			}}
			onClick={() => {
				// #271: a real drag's pointerup is followed by a click on the
				// same element — swallow that one so dropping doesn't also
				// select the tab. A plain click (no drag) passes straight through.
				if (suppressClick?.()) return;
				onClick();
			}}
			onDoubleClick={(e: ReactMouseEvent<HTMLDivElement>) => {
				// #241/#316: the dblclick handler lives on the TAB ROOT, not the
				// `.name` span, because useTabDrag's `startDrag` calls
				// `e.currentTarget.setPointerCapture` on every pointerdown on
				// this root — and in Chromium/WebView2, once this element has
				// pointer capture, the click/dblclick that follows is dispatched
				// to the CAPTURING element regardless of where the cursor
				// visually is, so `e.target` is always this div and a dblclick
				// handler on the span itself never fires. Hit-test the real
				// point instead (same `elementFromPoint` technique useTabDrag.ts
				// uses to find what's under the cursor during a drag) and only
				// start a rename if that point is actually over this tab's own
				// `.name` span.
				if (!onStartRename || renaming) return;
				const hit = document.elementFromPoint(e.clientX, e.clientY);
				const nameEl = hit instanceof Element ? hit.closest(".name") : null;
				if (!nameEl || !e.currentTarget.contains(nameEl)) return;
				onStartRename();
			}}
			data-drag-kind={dragKind}
			data-drag-id={dragId}
			data-drag-room={dragRoomId}
			data-drag-seg={dragSegId}
			data-drag-role={dragRole}
			onPointerDown={onPointerDown}
			onPointerMove={onPointerMove}
			onPointerUp={onPointerUp}
			onPointerCancel={onPointerCancel}
			onLostPointerCapture={onLostPointerCapture}
		>
			<div className="row-1">
				<StatusDot status={r.status} roomIds={[r.id]} aggName={r.name} />
				{/* #132: task tooltip lives on the name, not the whole tab, so
			    hovering a dot/chip shows only the status popover (not the
			    native tooltip on top of it). */}
				{renaming ? (
					<RoomNameInput
						initial={r.name}
						onCommit={(name) => {
							onRename?.(name);
							onRenameEnd?.();
						}}
						onCancel={() => onRenameEnd?.()}
					/>
				) : (
					// #241: dblclick-to-rename is wired on the tab ROOT, not here
					// — see its handler's comment for why.
					<span className="name" title={r.task}>
						{r.name}
					</span>
				)}
				{r.badge > 0 && <span className="tab-badge">{r.badge}</span>}
				{/* #328: a room created without switching to it (`createRoom`,
			    `{ activate: false }`) — a dot, not a count, since there is
			    nothing yet to count. Cleared the moment this room becomes
			    active, so it never shows on the tab you're already on. */}
				{r.attention && !active && <span className="tab-attention-dot" title="New room" />}
				{/* #241: hidden mid-rename — closing out from under the input
			    would archive the room `commit`/`onBlur` is about to write
			    a name onto, and a stray click here is an easy miss when the
			    span has just been replaced by an input in the same spot. */}
				{!renaming && (
					<span
						className="sk-tab-close"
						title="Close room"
						onClick={(e) => {
							e.stopPropagation();
							onClose();
						}}
					>
						×
					</span>
				)}
			</div>
			<div className="row-2">
				{r.branch && (
					<>
						<span className="branch" title={r.branch}>
							{r.branch}
						</span>
						<span>·</span>
					</>
				)}
				<span style={{ display: "flex", gap: 2 }}>
					{r.harnesses.map((h) => (
						<HChip key={h.id} kind={h.kind} harnessId={h.id} />
					))}
				</span>
			</div>
		</div>
	);
};
