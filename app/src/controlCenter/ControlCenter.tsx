// Control Center (#492): every open room, grouped like the strip and
// ranked by how much it needs the user. `ControlCenterView` is the pure
// renderer (also hosted by the #493 pop-out window); `ControlCenter` is the
// in-app wrapper that computes the rows.

import type { ReactNode } from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import { CCRowView } from "./CCRow.tsx";
import type { CCSection } from "./model.ts";
import { useControlCenterSections } from "./useControlCenterSections.ts";
import "./ControlCenter.css";

export interface ControlCenterViewProps {
	sections: readonly CCSection[];
	activeRoomId: string | null;
	now: number;
	onFocus: (roomId: string, harnessId?: string) => void;
	/** Toolbar rendered above the list (the in-app "Pop out" button). */
	header?: ReactNode;
	hidden?: boolean;
}

export function ControlCenterView({
	sections,
	activeRoomId,
	now,
	onFocus,
	header,
	hidden = false,
}: ControlCenterViewProps) {
	return (
		<div className="cc" hidden={hidden}>
			{hidden ? null : (
				<>
					{header}
					{sections.length === 0 ? (
						<div className="cc-empty">No open rooms.</div>
					) : (
						<div className="cc-list">
							<div className="cc-cols" aria-hidden="true">
								<span>Room</span>
								<span>Attention</span>
								<span>Driving</span>
								<span>Last status</span>
								<span>Review</span>
								<span>Active</span>
								<span className="cc-end">Harnesses</span>
							</div>
							{sections.map((sec) => (
								<section className="cc-section" key={sec.key} data-rank={sec.rank}>
									<header className="cc-group" data-group={sec.isGroup}>
										<span className="cc-group-label">
											{sec.isGroup ? sec.label : "Standalone room"}
										</span>
										{sec.isGroup ? (
											<span className="cc-group-count">
												{sec.rows.length} {sec.rows.length === 1 ? "room" : "rooms"}
											</span>
										) : null}
									</header>
									{sec.rows.map((row) => (
										<CCRowView
											key={row.room.id}
											row={row}
											active={row.room.id === activeRoomId}
											now={now}
											onFocus={onFocus}
										/>
									))}
								</section>
							))}
						</div>
					)}
				</>
			)}
		</div>
	);
}

export interface ControlCenterProps {
	rooms: Room[];
	segments: StripSegment[];
	activeRoomId: string | null;
	visible: boolean;
	onFocus: (roomId: string, harnessId?: string) => void;
	/** Moves the view into its own window (#493). */
	onPopOut?: () => void;
}

export function ControlCenter({
	rooms,
	segments,
	activeRoomId,
	visible,
	onFocus,
	onPopOut,
}: ControlCenterProps) {
	const { sections, now } = useControlCenterSections(rooms, segments, visible);
	const header = onPopOut ? (
		<div className="cc-toolbar">
			<button
				type="button"
				className="cc-popout-btn"
				title="Open the Control Center in its own window"
				onClick={onPopOut}
			>
				Pop out
			</button>
		</div>
	) : null;
	return (
		<ControlCenterView
			sections={rooms.length === 0 ? [] : sections}
			activeRoomId={activeRoomId}
			now={now}
			onFocus={onFocus}
			header={header}
			hidden={!visible}
		/>
	);
}
