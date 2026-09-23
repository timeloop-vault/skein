// App-level modals/overlays + the toast stack and the two DB-health
// banners (#19), moved out of App.tsx — pure move, no behaviour change.
// Rendered identically from both of App.tsx's `return`s (empty state
// and the normal layout), so this one component covers both.

import { CommandPalette, type PaletteItem } from "./CommandPalette.tsx";
import { NewRoomDialog } from "./NewRoomDialog.tsx";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import { ReopenRoomModal } from "./ReopenRoomModal.tsx";
import { SettingsModal } from "./SettingsModal.tsx";
import { Toast, type ToastEntry } from "./notifications.tsx";
import type { DefaultAgents, FolderDefaults, NewRoomMemory, RecentFolder } from "./prefs.ts";
import type { Room } from "./types.ts";

export const AppOverlays = ({
	showNewRoom,
	defaultCwd,
	newRoomSeed,
	defaultAgents,
	newRoomMemory,
	branchTemplate,
	recentRoomFolders,
	rememberRoomFolder,
	createRoom,
	setShowNewRoom,
	showPalette,
	paletteItems,
	setShowPalette,
	showSettings,
	settingsProps,
	showReopen,
	archivedRooms,
	reopenRoom,
	deleteRoomForever,
	restoreRoom,
	setShowReopen,
	toasts,
	jumpToToast,
	dismissToast,
	quarantinedCount,
	setQuarantinedCount,
	backupRoomCount,
	setBackupRoomCount,
}: {
	showNewRoom: boolean;
	defaultCwd: string;
	newRoomSeed: { cwd: string; defaults: FolderDefaults | undefined };
	defaultAgents: DefaultAgents;
	newRoomMemory: NewRoomMemory;
	branchTemplate: string;
	recentRoomFolders: RecentFolder[];
	rememberRoomFolder: (folder: string, defaults: Omit<FolderDefaults, "lastUsed">) => void;
	createRoom: (args: CreateRoomArgs) => void;
	setShowNewRoom: (show: boolean) => void;
	showPalette: boolean;
	paletteItems: PaletteItem[];
	setShowPalette: (show: boolean) => void;
	showSettings: boolean;
	settingsProps: React.ComponentProps<typeof SettingsModal>;
	showReopen: boolean;
	archivedRooms: Room[];
	reopenRoom: (id: string) => void;
	deleteRoomForever: (id: string) => void;
	restoreRoom: (room: Room) => void;
	setShowReopen: (show: boolean) => void;
	toasts: ToastEntry[];
	jumpToToast: (toast: ToastEntry) => void;
	dismissToast: (id: string) => void;
	quarantinedCount: number;
	setQuarantinedCount: (count: number) => void;
	backupRoomCount: number | null;
	setBackupRoomCount: (count: number | null) => void;
}) => {
	return (
		<>
			{showNewRoom && (
				<NewRoomDialog
					defaultCwd={defaultCwd}
					initialCwd={newRoomSeed.cwd}
					initialDefaults={newRoomSeed.defaults}
					defaultAgents={defaultAgents}
					memory={newRoomMemory}
					appBranchTemplate={branchTemplate}
					recent={recentRoomFolders}
					onRemember={rememberRoomFolder}
					onCommit={createRoom}
					onCancel={() => setShowNewRoom(false)}
				/>
			)}
			{showPalette && <CommandPalette items={paletteItems} onClose={() => setShowPalette(false)} />}
			{showSettings && <SettingsModal {...settingsProps} />}
			{showReopen && (
				<ReopenRoomModal
					rooms={archivedRooms}
					onReopen={reopenRoom}
					onDelete={deleteRoomForever}
					onRestore={restoreRoom}
					onClose={() => setShowReopen(false)}
				/>
			)}
			{/* L5c — toast stack. Floats above everything via `position:
			    fixed`; pointer-events on the container is `none` so it
			    doesn't catch clicks on the underlying app, while each
			    toast re-enables them. */}
			{toasts.length > 0 && (
				<div className="sk-toast-stack">
					{toasts.map((t) => (
						<Toast
							key={t.id}
							toast={t}
							onClick={() => jumpToToast(t)}
							onDismiss={() => dismissToast(t.id)}
						/>
					))}
				</div>
			)}
			{/* #167: some persisted rooms couldn't be parsed and were moved
			    to the quarantine table. Everything else loaded — surface
			    that instead of letting the shrunken room list masquerade
			    as normal. */}
			{quarantinedCount > 0 && (
				<div className="sk-quarantine-banner">
					<span>
						{quarantinedCount === 1
							? "1 saved room couldn't be read and was quarantined"
							: `${quarantinedCount} saved rooms couldn't be read and were quarantined`}
						{" — the rest loaded fine. The raw rows are preserved in skein.db "}
						(sessions_quarantine).
					</span>
					<span
						className="sk-quarantine-banner-x"
						title="Dismiss"
						onClick={() => setQuarantinedCount(0)}
					>
						×
					</span>
				</div>
			)}
			{/* #167: the live rooms table came back empty but the backup
			    next to it still holds rooms — say so instead of rendering
			    first-run onboarding over recoverable data. */}
			{backupRoomCount !== null && (
				<div className="sk-quarantine-banner">
					<span>
						{`Your rooms database is empty, but a backup holding ${backupRoomCount} room${
							backupRoomCount === 1 ? "" : "s"
						} sits next to it as skein.db.bak. To restore: quit Skein, copy skein.db.bak over `}
						skein.db, and delete skein.db-wal / skein.db-shm.
					</span>
					<span
						className="sk-quarantine-banner-x"
						title="Dismiss"
						onClick={() => setBackupRoomCount(null)}
					>
						×
					</span>
				</div>
			)}
		</>
	);
};
