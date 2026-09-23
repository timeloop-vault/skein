// Issue #164: rendered in place of a room's HarnessColumn when its
// folder has gone missing — nothing mounts under it, so no LiveTerminal
// spawns anywhere else. Reuses the #167 boot-error card look
// (`.sk-boot-error*` in styles.css).

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { type RepoRootInfo, recoveryOptions } from "./missingFolder.ts";
import type { Room } from "./types.ts";

interface MissingFolderCardProps {
	room: Room;
	/** Re-attach an existing branch as a worktree at `room.cwd` and, on
	 *  success, clear the room's missing flag. Only ever called when
	 *  `recoveryOptions` offers it. */
	onRecreateWorktree: () => Promise<{ ok: true } | { ok: false; error: string }>;
	/** Move the room to a freshly picked folder and clear the missing
	 *  flag. No-op if the dialog is cancelled. */
	onPickFolder: (newCwd: string) => void;
	onClose: () => void;
}

/** Just the room-body card; the folder picker dialog itself is invoked
 *  here (same call the New Room dialog makes) but the resulting
 *  cwd move is the caller's job — it needs `defaultShell` and the
 *  opencode port allocator that live in App.tsx. */
export const MissingFolderCard = ({
	room,
	onRecreateWorktree,
	onPickFolder,
	onClose,
}: MissingFolderCardProps) => {
	const [repoInfo, setRepoInfo] = useState<RepoRootInfo | undefined>(undefined);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	useEffect(() => {
		setRepoInfo(undefined);
		if (!room.repoRoot) return;
		let cancelled = false;
		void invoke<RepoRootInfo>("git_inspect_folder", { path: room.repoRoot })
			.then((info) => {
				if (!cancelled) setRepoInfo(info);
			})
			.catch((err: unknown) => {
				console.warn(`[skein] git_inspect_folder failed for repoRoot of room ${room.id}:`, err);
			});
		return () => {
			cancelled = true;
		};
	}, [room.id, room.repoRoot]);

	const recovery = recoveryOptions(room, repoInfo);

	const recreate = async () => {
		setBusy(true);
		setError(null);
		const result = await onRecreateWorktree();
		setBusy(false);
		if (!result.ok) setError(result.error);
	};

	const pick = async () => {
		const picked = await openDialog({
			directory: true,
			multiple: false,
			title: "Pick a folder for this room",
			...(room.cwd ? { defaultPath: room.cwd } : {}),
		});
		if (typeof picked === "string") onPickFolder(picked);
	};

	return (
		<div className="sk-boot-error">
			<div className="sk-boot-error-card">
				<div className="sk-boot-error-title">Folder missing</div>
				<div className="sk-boot-error-msg">{room.cwd}</div>
				{room.branch ? <div className="sk-boot-error-hint">Branch: {room.branch}</div> : null}
				<div className="sk-boot-error-hint">
					This folder no longer exists, so this room's harnesses won't run — Skein never spawns them
					somewhere else. Recreate the worktree, point the room at a different folder, or close it.
				</div>
				{!recovery.canRecreateWorktree ? (
					<div className="sk-boot-error-hint">{recovery.reason}</div>
				) : null}
				{error !== null ? <div className="sk-boot-error-msg">{error}</div> : null}
				<div className="sk-boot-error-actions">
					{recovery.canRecreateWorktree ? (
						<button
							type="button"
							className="sk-btn primary"
							disabled={busy}
							onClick={() => void recreate()}
						>
							Recreate worktree
						</button>
					) : null}
					<button type="button" className="sk-btn" disabled={busy} onClick={() => void pick()}>
						Pick another folder
					</button>
					<button type="button" className="sk-btn" disabled={busy} onClick={onClose}>
						Close room
					</button>
				</div>
			</div>
		</div>
	);
};
