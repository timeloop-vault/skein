// Control Center (#492): every open room, grouped like the strip and
// ranked by how much it needs the user. Props-only so it can be hosted
// elsewhere later (#493 pop-out).

import { useMemo } from "react";
import type { StripSegment } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import { CCRowView } from "./CCRow.tsx";
import { buildControlCenter } from "./model.ts";
import { useControlCenterData } from "./useControlCenterData.ts";
import "./ControlCenter.css";

export interface ControlCenterProps {
	rooms: Room[];
	segments: StripSegment[];
	activeRoomId: string | null;
	visible: boolean;
	onFocus: (roomId: string, harnessId?: string) => void;
}

export function ControlCenter({
	rooms,
	segments,
	activeRoomId,
	visible,
	onFocus,
}: ControlCenterProps) {
	const snaps = useControlCenterData(rooms, visible);
	const sections = useMemo(
		() => (visible ? buildControlCenter(segments, snaps) : []),
		[visible, segments, snaps],
	);
	// One instant for every row; `snaps` changes identity on each clock tick.
	// biome-ignore lint/correctness/useExhaustiveDependencies: recomputed when snaps change
	const now = useMemo(() => Date.now(), [snaps]);

	return (
		<div className="cc" hidden={!visible}>
			{!visible ? null : sections.length === 0 || rooms.length === 0 ? (
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
		</div>
	);
}
