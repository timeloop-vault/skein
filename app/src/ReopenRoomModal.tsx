// Reopen Room modal — chapter 6 phase 2, scaled up in #89.
//
// closeRoom (App.tsx) no longer deletes; it sets `archived = Date.now()`.
// This modal lists those archived rooms so a closed room can come back.
// Clicking a row clears the archived flag and switches the active room
// to it; the boot-time resume flow at App.tsx already handles re-spawning
// PTYs with their captured sessionIds, so the conversations come back too.
//
// #89: once you've daily-driven Skein for a while the list outgrows the
// screen and there's no way to prune it. So this now has a filter box, a
// bounded scrolling list, keyboard nav (↑/↓/Enter, Cmd/Ctrl+Backspace to
// delete), and a per-row "delete forever" with a brief in-modal undo.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Room } from "./types.ts";
import { useFocusRestore } from "./useFocusRestore.ts";

interface ReopenRoomModalProps {
	rooms: Room[]; // already filtered to archived + sorted newest-first
	onReopen: (id: string) => void;
	// #89: permanently remove an archived room from state (and, via the
	// autosave, from the DB).
	onDelete: (id: string) => void;
	// #89: re-insert a room dropped by `onDelete`, for the undo window.
	onRestore: (room: Room) => void;
	onClose: () => void;
}

// How long the "Deleted X · Undo" affordance stays around after a delete.
const UNDO_MS = 5_000;

const formatClosed = (ms: number): string => {
	const diff = Date.now() - ms;
	if (diff < 60_000) return "just now";
	if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
	if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} h ago`;
	if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} d ago`;
	return new Date(ms).toLocaleDateString();
};

export const ReopenRoomModal = ({
	rooms,
	onReopen,
	onDelete,
	onRestore,
	onClose,
}: ReopenRoomModalProps) => {
	useFocusRestore();
	const [query, setQuery] = useState("");
	const [selectedIdx, setSelectedIdx] = useState(0);
	const [pendingUndo, setPendingUndo] = useState<Room | null>(null);
	const inputRef = useRef<HTMLInputElement>(null);
	const selectedRef = useRef<HTMLDivElement>(null);
	const undoTimer = useRef<number | null>(null);

	// Autofocus the filter box so the user can type immediately.
	useEffect(() => {
		inputRef.current?.focus();
	}, []);

	// Don't leave a dangling undo timer if the modal unmounts.
	useEffect(
		() => () => {
			if (undoTimer.current !== null) clearTimeout(undoTimer.current);
		},
		[],
	);

	const filtered = useMemo(() => {
		const q = query.trim().toLowerCase();
		if (!q) return rooms;
		return rooms.filter((r) =>
			[r.name, r.repo, r.branch, r.cwd].some((f) => f?.toLowerCase().includes(q)),
		);
	}, [rooms, query]);

	// Keep the selection in range as the filter narrows the list.
	useEffect(() => {
		setSelectedIdx((idx) => (filtered.length === 0 ? 0 : Math.min(idx, filtered.length - 1)));
	}, [filtered.length]);

	// Keep the keyboard-selected row visible in the scrolling list.
	// selectedIdx is the intended trigger even though the body only reads
	// the ref — re-run the scroll whenever the selection moves.
	// biome-ignore lint/correctness/useExhaustiveDependencies: selectedIdx is the trigger; see comment.
	useEffect(() => {
		selectedRef.current?.scrollIntoView({ block: "nearest" });
	}, [selectedIdx]);

	const commitDelete = (room: Room) => {
		// A second delete supersedes the prior pending undo — that one
		// becomes permanent (matches the familiar single-slot undo).
		if (undoTimer.current !== null) clearTimeout(undoTimer.current);
		onDelete(room.id);
		setPendingUndo(room);
		undoTimer.current = window.setTimeout(() => {
			setPendingUndo(null);
			undoTimer.current = null;
		}, UNDO_MS);
	};

	const undo = () => {
		if (!pendingUndo) return;
		if (undoTimer.current !== null) clearTimeout(undoTimer.current);
		undoTimer.current = null;
		onRestore(pendingUndo);
		setPendingUndo(null);
	};

	const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
		if (e.key === "Escape") {
			e.preventDefault();
			onClose();
			return;
		}
		// Cmd/Ctrl+Backspace deletes the selected row; plain Backspace
		// stays text-editing in the filter box.
		if (e.key === "Backspace" && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			const room = filtered[selectedIdx];
			if (room) commitDelete(room);
			return;
		}
		// Cmd/Ctrl+Z undoes the most recent delete while its window is open.
		if (e.key === "z" && (e.metaKey || e.ctrlKey)) {
			if (pendingUndo) {
				e.preventDefault();
				undo();
			}
			return;
		}
		if (filtered.length === 0) return;
		if (e.key === "ArrowDown") {
			e.preventDefault();
			setSelectedIdx((idx) => (idx + 1) % filtered.length);
		} else if (e.key === "ArrowUp") {
			e.preventDefault();
			setSelectedIdx((idx) => (idx - 1 + filtered.length) % filtered.length);
		} else if (e.key === "Enter") {
			e.preventDefault();
			const room = filtered[selectedIdx];
			if (room) onReopen(room.id);
		}
	};

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Reopen room</h2>
					<div className="sub">Click to bring one back · trash to delete forever</div>
				</div>
				<input
					ref={inputRef}
					className="sk-reopen-search"
					placeholder="Filter by name, repo, branch, or path…"
					value={query}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				<div className="sk-modal-body sk-archived-list">
					{filtered.length === 0 && (
						<div className="sk-archived-empty">
							{rooms.length === 0 ? "No closed rooms to reopen." : "No rooms match your filter."}
						</div>
					)}
					{filtered.map((r, i) => (
						<div
							key={r.id}
							ref={i === selectedIdx ? selectedRef : null}
							className={`sk-archived-row ${i === selectedIdx ? "selected" : ""}`}
							onMouseEnter={() => setSelectedIdx(i)}
						>
							<button type="button" className="sk-archived-open" onClick={() => onReopen(r.id)}>
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
							<button
								type="button"
								className="sk-archived-del"
								title="Delete forever"
								aria-label={`Delete ${r.name} forever`}
								onClick={() => commitDelete(r)}
							>
								🗑
							</button>
						</div>
					))}
				</div>
				{pendingUndo && (
					<div className="sk-reopen-undo">
						<span className="msg">Deleted “{pendingUndo.name}”</span>
						<button type="button" className="sk-reopen-undo-btn" onClick={undo}>
							Undo
						</button>
					</div>
				)}
			</div>
		</div>
	);
};
