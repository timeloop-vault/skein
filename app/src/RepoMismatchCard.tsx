// Issue #418: rendered in place of a room's HarnessColumn when its
// folder now holds a different repository than the room was made for —
// nothing mounts under it, so no harness resumes into the wrong repo.
// Same card look as MissingFolderCard (`.sk-boot-error*`).

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { Room } from "./types.ts";
import "./boot.css";

interface RepoMismatchCardProps {
	room: Room;
	/** Archive + retire the room, keeping its history (#417). */
	onRetire: () => void;
	/** Move the room to a freshly picked folder; identity is re-checked. */
	onRepoint: (newCwd: string) => void;
	/** The user vouches the folder's repo is the room's: adopt its identity. */
	onSameRepo: () => void;
}

export const RepoMismatchCard = ({
	room,
	onRetire,
	onRepoint,
	onSameRepo,
}: RepoMismatchCardProps) => {
	const identity = room.repoIdentity;
	const shas = (identity?.rootCommits ?? []).map((c) => c.slice(0, 7));

	const pick = async () => {
		const picked = await openDialog({
			directory: true,
			multiple: false,
			title: "Pick a folder for this room",
			...(room.cwd ? { defaultPath: room.cwd } : {}),
		});
		if (typeof picked === "string") onRepoint(picked);
	};

	return (
		<div className="sk-boot-error">
			<div className="sk-boot-error-card">
				<div className="sk-boot-error-title">This folder now holds a different repository</div>
				<div className="sk-boot-error-msg">{room.cwd}</div>
				{identity?.originUrl ? (
					<div className="sk-boot-error-hint">This room was for {identity.originUrl}</div>
				) : null}
				{shas.length > 0 ? (
					<div className="sk-boot-error-hint">
						Root commit{shas.length > 1 ? "s" : ""}: {shas.join(", ")}
					</div>
				) : null}
				<div className="sk-boot-error-hint">
					Nothing was resumed: this room's harnesses stay stopped until you decide. Retire the room,
					point it at the right folder, or confirm it is the same repository.
				</div>
				<div className="sk-boot-error-actions">
					<button type="button" className="sk-btn primary" onClick={onRetire}>
						Retire this room
					</button>
					<button type="button" className="sk-btn" onClick={() => void pick()}>
						Repoint to…
					</button>
					<button type="button" className="sk-btn" onClick={onSameRepo}>
						It's the same repo
					</button>
				</div>
			</div>
		</div>
	);
};
