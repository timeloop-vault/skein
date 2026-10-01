import { HChip, StatusDot } from "./HChip.tsx";
import type { DragProps } from "./dragProps.ts";
import type { AgentLabel } from "./harnessAgent.ts";
import { requestDeliverNow } from "./mailDeliverNow.ts";
import { useMailHold } from "./mailHold.ts";
import { useUnreadMail } from "./mailStore.ts";
import type { Harness } from "./types.ts";

export const HarnessTab = ({
	h,
	agent,
	active,
	closable,
	onClick,
	onClose,
	dragging,
	dropSide,
	dragKind,
	dragId,
	dragRoomId,
	onPointerDown,
	onPointerMove,
	onPointerUp,
	onPointerCancel,
	onLostPointerCapture,
	suppressClick,
}: {
	h: Harness;
	/** #248: surfaced in the hover popover, not on the tab — the tab is
	 *  already dense. */
	agent?: AgentLabel | null;
	active: boolean;
	closable: boolean;
	onClick: () => void;
	onClose: () => void;
} & DragProps) => {
	// #329: the mailbox unread marker — read live so a message arriving
	// (or being read) updates the tab without a re-render trigger from
	// anywhere else.
	const mail = useUnreadMail(h.id);
	const hold = useMailHold(h.id);
	return (
		<div
			className={`sk-harness-tab ${active ? "active" : ""} ${dragging ? "dragging" : ""} ${dropSide ? `drop-${dropSide}` : ""}`}
			data-htab={h.id}
			onClick={() => {
				// #271: see RoomTab's onClick — same swallow-the-post-drop-click.
				if (suppressClick?.()) return;
				onClick();
			}}
			data-drag-kind={dragKind}
			data-drag-id={dragId}
			data-drag-room={dragRoomId}
			onPointerDown={onPointerDown}
			onPointerMove={onPointerMove}
			onPointerUp={onPointerUp}
			onPointerCancel={onPointerCancel}
			onLostPointerCapture={onLostPointerCapture}
		>
			<StatusDot status={h.status} />
			<HChip
				kind={h.kind}
				harnessId={h.id}
				agent={agent}
				mailCount={mail.count}
				mailFromRoomNames={mail.fromRoomNames}
			/>
			<span className="ht-name">{h.name}</span>
			{/* #329: no `title` here — the hover popover (statusPopover.ts)
			 *  shows the same mail info as a segment, matching every other
			 *  hover on this tab instead of a native tooltip. */}
			{mail.count > 0 && hold.held && (
				// #413: mail held by the composer-draft guard — click delivers now.
				<span
					className="tab-mail tab-mail--held"
					title={
						hold.releaseRefusal
							? `Deliver now refused — ${hold.releaseRefusal}`
							: "Mail held — the prompt may hold a draft. Click to deliver now."
					}
					onClick={(e) => {
						e.stopPropagation();
						void requestDeliverNow(h.id);
					}}
				>
					✉ {mail.count} held
				</span>
			)}
			{mail.count > 0 && !hold.held && <span className="tab-mail">✉ {mail.count}</span>}
			{closable && (
				<span
					className="ht-x"
					onClick={(e) => {
						e.stopPropagation();
						onClose();
					}}
				>
					×
				</span>
			)}
		</div>
	);
};
