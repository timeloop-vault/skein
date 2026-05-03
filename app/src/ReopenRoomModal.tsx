// Reopen Room modal — chapter 6 phase 2.
//
// closeRoom (App.tsx) no longer deletes; it sets `archived = Date.now()`.
// This modal lists those archived rooms so a closed room can come back.
// Clicking a row clears the archived flag and switches the active room
// to it; the boot-time resume flow at App.tsx already handles re-spawning
// PTYs with their captured sessionIds, so the conversations come back too.

import { useEffect } from "react";
import type { Room } from "./types.ts";

interface ReopenRoomModalProps {
	rooms: Room[]; // already filtered to archived + sorted newest-first
	onReopen: (id: string) => void;
	onClose: () => void;
}

const formatClosed = (ms: number): string => {
	const diff = Date.now() - ms;
	if (diff < 60_000) return "just now";
	if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
	if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} h ago`;
	if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} d ago`;
	return new Date(ms).toLocaleDateString();
};

export const ReopenRoomModal = ({ rooms, onReopen, onClose }: ReopenRoomModalProps) => {
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				onClose();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [onClose]);

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Reopen room</h2>
					<div className="sub">Recently closed rooms. Click to bring one back.</div>
				</div>
				<div className="sk-modal-body sk-archived-list">
					{rooms.length === 0 && (
						<div className="sk-archived-empty">No closed rooms to reopen.</div>
					)}
					{rooms.map((r) => (
						<button
							key={r.id}
							type="button"
							className="sk-archived-row"
							onClick={() => onReopen(r.id)}
						>
							<div className="top">{r.name}</div>
							<div className="meta">
								{r.repo && r.branch ? (
									<span>
										{r.repo} · {r.branch}
									</span>
								) : (
									<span>{r.cwd ?? "no cwd"}</span>
								)}
								<span className="age">{r.archived ? formatClosed(r.archived) : ""}</span>
							</div>
						</button>
					))}
				</div>
			</div>
		</div>
	);
};
