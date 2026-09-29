// Reopen Room modal — chapter 6 phase 2, scaled up in #89.
//
// closeRoom (App.tsx) no longer deletes; it sets `archived = Date.now()`.
// This modal lists those archived rooms so a closed room can come back.
// Reopening a row calls App's `unarchiveRoom`, which clears the archived
// flag *and* rewrites every harness cmd into resume form before the
// remount respawns it. Both halves matter: the boot-time resume flow
// does not run again here, and assuming it did is what shipped #153 and
// then #170.
//
// #89: once you've daily-driven Skein for a while the list outgrows the
// screen and there's no way to prune it. So this has a filter box, a
// bounded scrolling list, keyboard nav (↑/↓/Enter, Cmd/Ctrl+Backspace to
// delete), and a delete-forever with a brief in-modal undo.
//
// #417: multi-select (click / Ctrl-click / Shift-click / Shift+arrows),
// bulk Retire and Delete forever, and "Show retired" with un-retire. The
// keyboard cursor (`selectedIdx`) is separate from the selection; the
// pure list logic lives in reopenList.ts.

import { useEffect, useMemo, useRef, useState } from "react";
import { confirmDialog } from "./confirmDialog.ts";
import {
	type Selection,
	click,
	emptySelection,
	extendTo,
	pruneTo,
	selectAll,
	toggle,
	visibleArchived,
} from "./reopenList.ts";
import type { Room } from "./types.ts";
import { useFocusRestore } from "./useFocusRestore.ts";

interface ReopenRoomModalProps {
	// Every archived room, retired included, sorted newest-first; the modal
	// hides retired ones itself unless "Show retired" is on (#417).
	rooms: Room[];
	// #411: every room (active + archived), for resolving a `closedBy`
	// attribution to a display name — the closing room is very often
	// still active (a planning room), so `rooms` above isn't enough.
	allRooms: Room[];
	onReopen: (id: string) => void;
	// #89: permanently remove archived rooms from state (and, via the
	// autosave, from the DB).
	onDelete: (ids: readonly string[]) => void;
	// #89: re-insert rooms dropped by `onDelete`, for the undo window.
	onRestore: (rooms: Room[]) => void;
	// #417: hide from the list without deleting; reversible via Show retired.
	onRetire: (ids: readonly string[]) => void;
	onUnretire: (ids: readonly string[]) => void;
	onClose: () => void;
}

interface PendingUndo {
	message: string;
	run: () => void;
}

// How long the "Deleted X · Undo" affordance stays around after an action.
const UNDO_MS = 5_000;

const formatClosed = (ms: number): string => {
	const diff = Date.now() - ms;
	if (diff < 60_000) return "just now";
	if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
	if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} h ago`;
	if (diff < 7 * 86_400_000) return `${Math.floor(diff / 86_400_000)} d ago`;
	return new Date(ms).toLocaleDateString();
};

// #411: "3 h ago" or, when the `close_room` agent verb closed it,
// "3 h ago · closed by <room>". Folded into one string (rather than a
// separate element) so the existing two-child `.meta` row's
// `space-between` layout doesn't need to change for a third. Falls
// back to the bare id if the closing room itself is gone (deleted
// forever since).
const closedLabel = (room: Room, allRooms: Room[]): string => {
	if (!room.archived) return "";
	const age = formatClosed(room.archived);
	if (!room.closedBy) return age;
	const name = allRooms.find((r) => r.id === room.closedBy?.roomId)?.name ?? room.closedBy.roomId;
	return `${age} · closed by ${name}`;
};

const noun = (n: number): string => `${n} room${n === 1 ? "" : "s"}`;

export const ReopenRoomModal = ({
	rooms,
	allRooms,
	onReopen,
	onDelete,
	onRestore,
	onRetire,
	onUnretire,
	onClose,
}: ReopenRoomModalProps) => {
	useFocusRestore();
	const [query, setQuery] = useState("");
	const [showRetired, setShowRetired] = useState(false);
	// The keyboard cursor is separate from the multi-selection (#417).
	const [selectedIdx, setSelectedIdx] = useState(0);
	const [sel, setSel] = useState<Selection>(emptySelection);
	const [pendingUndo, setPendingUndo] = useState<PendingUndo | null>(null);
	const inputRef = useRef<HTMLInputElement>(null);
	const selectedRef = useRef<HTMLDivElement>(null);
	const undoTimer = useRef<number | null>(null);
	// A delete awaiting its confirm dialog; a second trigger meanwhile is ignored.
	const deleting = useRef(false);

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

	const filtered = useMemo(
		() => visibleArchived(rooms, query, showRetired),
		[rooms, query, showRetired],
	);
	const ids = useMemo(() => filtered.map((r) => r.id), [filtered]);

	// Keep the cursor in range and the selection to visible rows as the
	// filter / Show retired / the room list change.
	useEffect(() => {
		setSelectedIdx((idx) => (ids.length === 0 ? 0 : Math.min(idx, ids.length - 1)));
		setSel((s) => pruneTo(s, ids));
	}, [ids]);

	// Keep the cursor row visible in the scrolling list.
	// biome-ignore lint/correctness/useExhaustiveDependencies: selectedIdx is the trigger; the body only reads the ref.
	useEffect(() => {
		selectedRef.current?.scrollIntoView({ block: "nearest" });
	}, [selectedIdx]);

	const cursorRoom = filtered[selectedIdx];
	// What a toolbar action / shortcut acts on: the selection, else the cursor row.
	const targets: Room[] =
		sel.ids.size > 0 ? filtered.filter((r) => sel.ids.has(r.id)) : cursorRoom ? [cursorRoom] : [];
	const retirable = targets.filter((r) => r.retired === undefined);

	const offerUndo = (next: PendingUndo) => {
		// A newer action supersedes the prior pending undo (single slot).
		if (undoTimer.current !== null) clearTimeout(undoTimer.current);
		setPendingUndo(next);
		undoTimer.current = window.setTimeout(() => {
			setPendingUndo(null);
			undoTimer.current = null;
		}, UNDO_MS);
	};

	const undo = () => {
		if (!pendingUndo) return;
		if (undoTimer.current !== null) clearTimeout(undoTimer.current);
		undoTimer.current = null;
		pendingUndo.run();
		setPendingUndo(null);
	};

	const retire = (list: Room[]) => {
		const rs = list.filter((r) => r.retired === undefined);
		if (rs.length === 0) return;
		const rIds = rs.map((r) => r.id);
		onRetire(rIds);
		offerUndo({ message: `Retired ${noun(rs.length)}`, run: () => onUnretire(rIds) });
	};

	const deleteForever = async (list: Room[]) => {
		if (list.length === 0 || deleting.current) return;
		// A retired room's history goes with it, so that is never a quiet delete.
		const hasRetired = list.some((r) => r.retired !== undefined);
		if (list.length > 1 || hasRetired) {
			deleting.current = true;
			let ok: boolean;
			try {
				ok = await confirmDialog({
					title: "Delete forever",
					message: `Delete ${noun(list.length)} forever? Their history (activity feed, phase history, review threads, sign-off and mail) is erased permanently.`,
					confirmLabel: `Delete ${noun(list.length)}`,
					cancelLabel: "Cancel",
					kind: "warning",
				});
			} catch (err) {
				console.error("[skein] confirmDialog failed:", err);
				return;
			} finally {
				deleting.current = false;
			}
			if (!ok) return;
		}
		onDelete(list.map((r) => r.id));
		offerUndo({
			message: list.length === 1 ? `Deleted “${list[0]?.name}”` : `Deleted ${noun(list.length)}`,
			run: () => onRestore(list),
		});
		inputRef.current?.focus();
	};

	const activate = (r: Room) => {
		if (r.retired !== undefined) onUnretire([r.id]);
		else onReopen(r.id);
	};

	// Shift-extend needs an anchor; with none yet, start from the cursor row.
	const extend = (id: string): Selection => {
		const base = sel.anchor === null && cursorRoom ? click(sel, cursorRoom.id) : sel;
		return extendTo(base, ids, id);
	};

	const onRowClick = (e: React.MouseEvent, i: number, id: string) => {
		setSelectedIdx(i);
		if (e.shiftKey) setSel(extend(id));
		else if (e.ctrlKey || e.metaKey) setSel(toggle(sel, id));
		else setSel(click(sel, id));
		inputRef.current?.focus();
	};

	const move = (e: React.KeyboardEvent, next: number) => {
		const id = ids[next];
		if (id === undefined) return;
		setSelectedIdx(next);
		setSel(e.shiftKey ? extend(id) : click(sel, id));
	};

	const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
		if (e.key === "Escape") {
			e.preventDefault();
			onClose();
			return;
		}
		// Cmd/Ctrl+Backspace deletes the selection (or cursor row); plain
		// Backspace stays text-editing in the filter box.
		if (e.key === "Backspace" && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void deleteForever(targets);
			return;
		}
		// Cmd/Ctrl+Z undoes the most recent action while its window is open.
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
			move(e, (selectedIdx + 1) % filtered.length);
		} else if (e.key === "ArrowUp") {
			e.preventDefault();
			move(e, (selectedIdx - 1 + filtered.length) % filtered.length);
		} else if (e.key === "Enter") {
			e.preventDefault();
			if (cursorRoom) activate(cursorRoom);
		}
	};

	// Keep focus in the filter box when a button is pressed, so the
	// keyboard shortcuts keep working afterwards.
	const keepFocus = (e: React.MouseEvent) => e.preventDefault();

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>Reopen room</h2>
					<div className="sub">
						Click to select · Ctrl/Cmd- or Shift-click for several · Enter reopens (un-retires a
						retired row)
					</div>
				</div>
				<input
					ref={inputRef}
					className="sk-reopen-search"
					placeholder="Filter by name, repo, branch, or path…"
					value={query}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				<div className="sk-reopen-toolbar">
					<button
						type="button"
						className="sk-reopen-tool"
						disabled={filtered.length === 0}
						onMouseDown={keepFocus}
						onClick={() => setSel(selectAll(ids))}
					>
						Select all {filtered.length} matching
					</button>
					<span className="sk-reopen-count">{sel.ids.size} selected</span>
					<span className="sk-reopen-spacer" />
					<label className="sk-reopen-toggle">
						<input
							type="checkbox"
							checked={showRetired}
							onChange={(e) => setShowRetired(e.target.checked)}
						/>
						Show retired
					</label>
					<button
						type="button"
						className="sk-reopen-tool"
						disabled={retirable.length === 0}
						onMouseDown={keepFocus}
						onClick={() => retire(targets)}
					>
						Retire
					</button>
					<button
						type="button"
						className="sk-reopen-tool danger"
						disabled={targets.length === 0}
						onMouseDown={keepFocus}
						onClick={() => void deleteForever(targets)}
					>
						Delete forever
					</button>
				</div>
				<div className="sk-modal-body sk-archived-list">
					{filtered.length === 0 && (
						<div className="sk-archived-empty">
							{rooms.length === 0 ? "No closed rooms to reopen." : "No rooms match your filter."}
						</div>
					)}
					{filtered.map((r, i) => {
						const retired = r.retired !== undefined;
						return (
							<div
								key={r.id}
								ref={i === selectedIdx ? selectedRef : null}
								className={`sk-archived-row ${i === selectedIdx ? "selected" : ""} ${sel.ids.has(r.id) ? "checked" : ""} ${retired ? "retired" : ""}`}
							>
								<button
									type="button"
									className="sk-archived-open"
									onClick={(e) => onRowClick(e, i, r.id)}
									onDoubleClick={() => activate(r)}
								>
									<div className="top">
										{r.name}
										{retired && <span className="sk-archived-tag">retired</span>}
									</div>
									<div className="meta">
										{r.repo && r.branch ? (
											<span>
												{r.repo} · {r.branch}
											</span>
										) : (
											<span>{r.cwd ?? "no cwd"}</span>
										)}
										<span className="age">{closedLabel(r, allRooms)}</span>
									</div>
								</button>
								<button
									type="button"
									className="sk-archived-act"
									onMouseDown={keepFocus}
									onClick={() => activate(r)}
								>
									{retired ? "Un-retire" : "Reopen"}
								</button>
								<button
									type="button"
									className="sk-archived-del"
									title="Delete forever"
									aria-label={`Delete ${r.name} forever`}
									onMouseDown={keepFocus}
									onClick={() => void deleteForever([r])}
								>
									🗑
								</button>
							</div>
						);
					})}
				</div>
				{pendingUndo && (
					<div className="sk-reopen-undo">
						<span className="msg">{pendingUndo.message}</span>
						<button type="button" className="sk-reopen-undo-btn" onClick={undo}>
							Undo
						</button>
					</div>
				)}
			</div>
		</div>
	);
};
