// Skein interactive prototype — single React tree.
//
// Mental model:
//   - Tabs along the top are rooms (repo + branch + task + cwd).
//   - Each room owns N harnesses (Claude Code, opencode, gh copilot,
//     or a built-in shell). All harnesses in a room share the same
//     worktree.
//   - The right pane belongs to the room, not the harness — so when
//     you switch agents inside the same room, the diff and status
//     stay put.

import { useEffect, useRef } from "react";
import { EmptyState, Titlebar, type TitlebarProps } from "./AppChrome.tsx";
import { AppOverlays } from "./AppOverlays.tsx";
import { AppShell, BootBody } from "./AppShell.tsx";
import { buildSettingsProps } from "./buildSettingsProps.ts";
import type { PaletteItem } from "./CommandPalette.tsx";
import { StatusDot } from "./components.tsx";
import { usePermissionHarnessIds } from "./harnessActivity.ts";
import { buildPaletteItems } from "./paletteItems.ts";
import { RoomTabStrip } from "./RoomTabStrip.tsx";
import { RoomWorkspace } from "./RoomWorkspace.tsx";
import { StatusBar } from "./StatusBar.tsx";
import { useAppBackgroundWiring } from "./useAppBackgroundWiring.ts";
import { useAppSettings } from "./useAppSettings.ts";
import { useAppShortcuts } from "./useAppShortcuts.ts";
import { useAppUiState } from "./useAppUiState.ts";
import { useHarnessActions } from "./useHarnessActions.ts";
import { useHarnessCreation } from "./useHarnessCreation.ts";
import { useHarnessNotifications } from "./useHarnessNotifications.ts";
import { useReattachTelemetry } from "./useReattachTelemetry.ts";
import { useRoomStripNav } from "./useRoomStripNav.ts";
import { useRoomsStore } from "./useRoomsStore.ts";
import { useTabDrag } from "./useTabDrag.ts";

// ── App ────────────────────────────────────────────────────────────

export default function App() {
	// #19: app-wide settings/prefs state — see useAppSettings.ts.
	const settings = useAppSettings();
	const { theme, density, chromeFontPt } = settings;
	// #86: every harness currently blocked on a permission dialog,
	// across every room — the status-bar urgent slot ranks these above
	// a plain pending-notifications backlog.
	const permissionHarnessIds = usePermissionHarnessIds();
	// Overlay flags, rename target, boot defaults, window effects, spawn env.
	const {
		showPicker,
		setShowPicker,
		showPalette,
		setShowPalette,
		showSettings,
		setShowSettings,
		showReopen,
		setShowReopen,
		renaming,
		setRenaming,
		defaultShell,
		defaultCwd,
		popoverRoomsRef,
		lastUsedByGroupRef,
		spawnEnv,
		saveSpawnSettings,
	} = useAppUiState();

	// #19: room lifecycle + persistence state — see useRoomsStore.ts.
	const store = useRoomsStore(defaultShell, setRenaming, lastUsedByGroupRef);
	const {
		setRooms,
		roomsRef,
		activeRoomId,
		setActiveRoomId,
		activeRoomIdRef,
		setOpencodePorts,
		loaded,
		loadFailed,
		setLoadFailed,
		checkRoomFolder,
		hydrateRooms,
		activeRooms,
		archivedRooms,
		archivedRoomsRef,
		room,
		activeHarness,
		unarchiveRoom,
		unarchiveRoomRef,
		closeRoom,
		switchRoom,
	} = store;

	// Keyboard nav (Mod+Tab, Mod+1..9) keys off active rooms only —
	// archived ones aren't rendered as tabs.
	const activeRoomsRef = useRef(activeRooms);
	activeRoomsRef.current = activeRooms;
	// #331: keep in sync for the status-popover breakdown.
	popoverRoomsRef.current = roomsRef.current;

	// #19: room-strip navigation (drag-reorder, segments, group "last
	// used" memory, tab click resolution) and New Room opening — see
	// useRoomStripNav.ts. `reorderRoom`/`reorderHarness` feed `useTabDrag`
	// below, and `setShowNewRoom` feeds `useHarnessCreation`.
	const nav = useRoomStripNav(
		activeRooms,
		archivedRoomsRef,
		roomsRef,
		activeRoomId,
		activeRoomIdRef,
		activeRoomsRef,
		setRooms,
		switchRoom,
		unarchiveRoomRef,
		lastUsedByGroupRef,
		store.mismatchedRooms,
	);
	const {
		reorderRoom,
		reorderHarness,
		stripSegments,
		activeSegment,
		onSelectSegment,
		setShowNewRoom,
		newRoomMemory,
		openNewRoom,
		openNewRoomAt,
		openGroupPlaceholder,
	} = nav;

	// #19: harness/room action helpers — see useHarnessActions.ts. Called
	// before useHarnessCreation, which needs `switchHarnessInRoom`.
	const actions = useHarnessActions(
		setRooms,
		setActiveRoomId,
		activeRoomId,
		activeRooms,
		setShowPicker,
		setRenaming,
	);
	const {
		startRenameRoom,
		endRenameRoom,
		commitRenameRoom,
		switchHarnessInRoom,
		jumpToHarness,
		cycleAlertedRoom,
		cycleAlertedHarness,
		addHarness,
	} = actions;

	const reopenRoom = async (id: string) => {
		await unarchiveRoom(id);
		setShowReopen(false);
	};

	// #19: harness/room creation — see useHarnessCreation.ts.
	const creation = useHarnessCreation(
		roomsRef,
		setRooms,
		setOpencodePorts,
		setActiveRoomId,
		activeRoomIdRef,
		defaultShell,
		defaultCwd,
		showPicker,
		setShowPicker,
		setShowNewRoom,
		switchHarnessInRoom,
	);
	const { createRoom, replaceHarnessSessionId } = creation;

	// #19: the notification engine (badge/toast/OS notifications, permission
	// and session-start listeners, transition logging) — see
	// useHarnessNotifications.ts. `displayedHarnessId` is the (active room,
	// active harness) tuple its clear-pending-on-view effect watches.
	const displayedHarnessId = room?.activeHarnessId ?? null;
	const { toasts, dismissToast, jumpToToast, pushToast } = useHarnessNotifications(
		roomsRef,
		activeRoomIdRef,
		setRooms,
		activeRoomId,
		displayedHarnessId,
		settings.notifyBadge,
		settings.notifyToast,
		settings.notifyOs,
		replaceHarnessSessionId,
		jumpToHarness,
	);

	// #410: manual "Reattach telemetry" action — see useReattachTelemetry.ts.
	const onReattachTelemetry = useReattachTelemetry(roomsRef, pushToast);

	// Pointer-based drag-to-reorder (#271) — see tabDrag.ts's header.
	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = useTabDrag(
		reorderRoom,
		reorderHarness,
	);

	// #164: re-check the active room's folder every time it becomes
	// active — covers a folder deleted (or a worktree removed) while
	// Skein was pointed at a different tab, which no watcher tells us
	// about. Hydrate and unarchive cover the other two ways a room
	// starts being rendered.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const room = roomsRef.current.find((r) => r.id === activeRoomId);
		if (room) void checkRoomFolder(room);
	}, [activeRoomId, checkRoomFolder]);

	// #19: window-level keyboard shortcuts + the handler refs they (and
	// the palette) read — see useAppShortcuts.ts.
	const { toggleFilesRef, toggleReviewRef } = useAppShortcuts({
		store,
		nav,
		actions,
		creation,
		settings,
		activeRoomsRef,
		lastUsedByGroupRef,
		setShowPalette,
		setShowSettings,
	});

	useAppBackgroundWiring({ store, nav, actions, creation, settings, pushToast });

	const titlebarProps: TitlebarProps = {
		activeRoomLabel: room ? room.name : null,
		onOpenSettings: () => setShowSettings(true),
	};

	const settingsProps = buildSettingsProps(
		settings,
		spawnEnv,
		saveSpawnSettings,
		room?.cwd ?? defaultCwd,
		() => setShowSettings(false),
	);

	// Phase 4 / #19: items the command palette offers (paletteItems.ts).
	// Still called as a plain function every render, not useMemo'd — see
	// that module's header for why.
	const paletteItems: PaletteItem[] = buildPaletteItems({
		activeRooms,
		archivedRooms,
		room,
		activeHarness,
		activeRoomId,
		theme,
		setActiveRoomId,
		setRooms,
		setTheme: settings.setTheme,
		setShowReopen,
		openNewRoom,
		toggleFilesRef,
		toggleReviewRef,
		addHarness,
		startRenameRoom,
		closeRoom,
		cycleAlertedRoom,
		cycleAlertedHarness,
		onReattachTelemetry,
	});

	const overlayProps = {
		showNewRoom: nav.showNewRoom,
		defaultCwd,
		newRoomSeed: nav.newRoomSeed,
		defaultAgents: settings.defaultAgents,
		newRoomMemory,
		branchTemplate: settings.branchTemplate,
		recentRoomFolders: nav.recentRoomFolders,
		rememberRoomFolder: nav.rememberRoomFolder,
		createRoom,
		setShowNewRoom,
		showPalette,
		paletteItems,
		setShowPalette,
		showSettings,
		settingsProps,
		showReopen,
		archivedRooms,
		allRooms: roomsRef.current,
		reopenRoom,
		deleteRoomsForever: store.deleteRoomsForever,
		restoreRooms: store.restoreRooms,
		retireRooms: store.retireRooms,
		unretireRooms: store.unretireRooms,
		setShowReopen,
		toasts,
		jumpToToast,
		dismissToast,
		quarantinedCount: store.quarantinedCount,
		setQuarantinedCount: store.setQuarantinedCount,
		backupRoomCount: store.backupRoomCount,
		setBackupRoomCount: store.setBackupRoomCount,
	};
	const dragWiring = { drag, dropTarget, startDrag, dragHandlers, suppressClick };

	// Pre-hydration: rooms haven't loaded yet, so `activeRooms` is
	// transiently [] — a quiet shell, never the EmptyState (#39). The
	// auto-save effect is parked on !loaded, so this never writes over the
	// unread DB.
	if (!loaded) {
		return (
			<AppShell theme={theme} density={density} chromeFontPt={chromeFontPt}>
				<BootBody
					titlebarProps={titlebarProps}
					loadFailed={loadFailed}
					onRetry={() => {
						setLoadFailed(null);
						hydrateRooms();
					}}
				/>
			</AppShell>
		);
	}

	// Empty state — no *active* rooms. Archived rooms still in the list
	// show via the reopen modal (linked from the empty state too).
	if (activeRooms.length === 0) {
		return (
			<AppShell theme={theme} density={density} chromeFontPt={chromeFontPt}>
				<Titlebar {...titlebarProps} />
				<EmptyState
					onNew={() => void openNewRoom()}
					archivedCount={archivedRooms.filter((r) => r.retired === undefined).length}
					onReopen={() => setShowReopen(true)}
				/>
				<AppOverlays {...overlayProps} />
			</AppShell>
		);
	}

	if (!room || !activeHarness) {
		// Shouldn't happen in practice: rooms is non-empty above.
		return null;
	}

	return (
		<AppShell theme={theme} density={density} chromeFontPt={chromeFontPt}>
			<Titlebar {...titlebarProps} />
			<RoomTabStrip
				stripSegments={stripSegments}
				activeSegment={activeSegment}
				activeRoomId={activeRoomId}
				onSelectSegment={onSelectSegment}
				closeRoom={closeRoom}
				openNewRoom={openNewRoom}
				openNewRoomAt={openNewRoomAt}
				openGroupPlaceholder={openGroupPlaceholder}
				switchRoom={switchRoom}
				dragWiring={dragWiring}
				renaming={renaming}
				onStartRename={startRenameRoom}
				onRename={commitRenameRoom}
				onRenameEnd={endRenameRoom}
			/>
			<RoomWorkspace
				activeRooms={activeRooms}
				activeRoomId={activeRoomId}
				store={store}
				actions={actions}
				creation={creation}
				settings={settings}
				defaultShell={defaultShell}
				showPicker={showPicker}
				setShowPicker={setShowPicker}
				drag={dragWiring}
				onReattachTelemetry={onReattachTelemetry}
			/>
			<StatusBar
				activeHarness={activeHarness}
				room={room}
				liveBranches={settings.liveBranches}
				notifyUrgent={settings.notifyUrgent}
				activeRooms={activeRooms}
				activeRoomId={activeRoomId}
				permissionHarnessIds={permissionHarnessIds}
				jumpToHarness={jumpToHarness}
				switchRoom={switchRoom}
			/>
			<AppOverlays {...overlayProps} />
		</AppShell>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
