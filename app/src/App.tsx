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

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { EmptyState, Titlebar, type TitlebarProps } from "./AppChrome.tsx";
import { AppOverlays } from "./AppOverlays.tsx";
import type { PaletteItem } from "./CommandPalette.tsx";
import { HarnessColumn } from "./HarnessColumn.tsx";
import { MissingFolderCard } from "./MissingFolderCard.tsx";
import { RightPane } from "./RightPane.tsx";
import { GroupRow, type RenameTarget, RoomStrip } from "./RoomStrip.tsx";
import { Splitter } from "./Splitter.tsx";
import { StatusBar } from "./StatusBar.tsx";
import { StatusDot } from "./components.tsx";
import { usePermissionHarnessIds } from "./harnessActivity.ts";
import { buildPaletteItems } from "./paletteItems.ts";
import { withDefaultAgent } from "./prefs.ts";
import { allRoomOrder } from "./roomGroups.ts";
import { isMac } from "./shortcuts.ts";
import type { HarnessKind, Room, SpawnSettings, SpawnSettingsPayload } from "./types.ts";
import {
	CHROME_FONT_MAX,
	CHROME_FONT_MIN,
	FONT_MAX,
	FONT_MIN,
	useAppSettings,
} from "./useAppSettings.ts";
import { useAppWindowEffects } from "./useAppWindowEffects.ts";
import { useHarnessActions } from "./useHarnessActions.ts";
import { useHarnessCreation } from "./useHarnessCreation.ts";
import { useHarnessNotifications } from "./useHarnessNotifications.ts";
import { useKeyboardShortcuts } from "./useKeyboardShortcuts.ts";
import { useOsNotificationClicks } from "./useOsNotificationClicks.ts";
import { useRoomStripNav } from "./useRoomStripNav.ts";
import { useRoomsStore } from "./useRoomsStore.ts";
import { useTabDrag } from "./useTabDrag.ts";

// ── App ────────────────────────────────────────────────────────────

export default function App() {
	// #19: app-wide settings/prefs state, extracted to keep this file's
	// growth minimal — see useAppSettings.ts.
	const {
		theme,
		setTheme,
		density,
		setDensity,
		fontSize,
		setFontSize,
		chromeFontPt,
		setChromeFontPt,
		copyOnSelect,
		setCopyOnSelect,
		notifyBadge,
		setNotifyBadge,
		notifyToast,
		setNotifyToast,
		notifyUrgent,
		setNotifyUrgent,
		notifyOs,
		setNotifyOs,
		showTurnCosts,
		handleToggleTurnCosts,
		defaultAgents,
		setDefaultAgents,
		branchTemplate,
		setBranchTemplate,
		rightPaneTabs,
		setRightPaneTab,
		harnessColWidth,
		setHarnessColWidth,
		liveBranches,
		handleBranchChange,
	} = useAppSettings();
	// #86: every harness currently blocked on a permission dialog,
	// across every room — the status-bar urgent slot ranks these above
	// a plain pending-notifications backlog.
	const permissionHarnessIds = usePermissionHarnessIds();
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [showPalette, setShowPalette] = useState(false);
	const [showSettings, setShowSettings] = useState(false);
	const [showReopen, setShowReopen] = useState(false);
	// #241: which room (if any) is mid inline-rename, and which tab hosts
	// the input (`RenameTarget.host` — see RoomStrip.tsx: a group's main
	// room can be shown by both the top-row `GroupTab` and its own
	// second-row lead tab, and only one may mount the input at a time).
	// Display only — commit touches `Room.name` alone, never
	// branch/cwd/worktree/repoRoot.
	const [renaming, setRenaming] = useState<RenameTarget | null>(null);

	// Phase 1: pull platform defaults once at boot. New harnesses spawn
	// into these until Phase 4 wires real worktrees / per-room cwd.
	const [defaultShell, setDefaultShell] = useState<string[]>([]);
	const [defaultCwd, setDefaultCwd] = useState<string>("");
	// #331: `rooms` (below, via useRoomsStore) for the status-popover
	// breakdown — created here, before useRoomsStore runs, because
	// useAppWindowEffects (which attaches the popover) mounts first;
	// kept in sync just after useRoomsStore returns.
	const popoverRoomsRef = useRef<readonly Room[]>([]);
	// #76: the room last used in each group (see useRoomStripNav.ts) —
	// created here, before useRoomsStore, so #334's closeRoom can read
	// it too; useRoomStripNav still owns writing to it.
	const lastUsedByGroupRef = useRef<Map<string, string>>(new Map());
	// #19: six standalone window/app-level effects — the boot-time
	// default-shell/default-cwd probe above, the quit-confirmation
	// wiring, the Esc-closes-picker listener, the skein://open-settings
	// listener, the #132 status-popover attach, and the #120 stray-
	// file-drop swallow — extracted to keep this file's growth minimal;
	// none of the six interacts with anything else here — see
	// useAppWindowEffects.ts.
	useAppWindowEffects(
		showPicker,
		setShowPicker,
		setShowSettings,
		setDefaultShell,
		setDefaultCwd,
		() => popoverRoomsRef.current,
	);

	// Shell / PATH environment (#72, #3, #1). Owned by Rust — the spawn
	// path reads it and the shell probe runs during setup(), before this
	// webview exists — so App.tsx only mirrors it for the Settings UI
	// and never feeds it back into a spawn.
	const [spawnEnv, setSpawnEnv] = useState<SpawnSettingsPayload | null>(null);
	useEffect(() => {
		void invoke<SpawnSettingsPayload>("spawn_settings_load").then(setSpawnEnv);
	}, []);
	const saveSpawnSettings = useCallback(async (next: SpawnSettings) => {
		const payload = await invoke<SpawnSettingsPayload>("spawn_settings_save", {
			settings: next,
		});
		setSpawnEnv(payload);
		// The shell may have changed, and `defaultShell` is what new
		// Shell harnesses and the Enter-for-shell prompt spawn.
		setDefaultShell(await invoke<string[]>("default_shell"));
	}, []);

	// #19: room lifecycle + persistence state, extracted to keep this
	// file's growth minimal — see useRoomsStore.ts.
	const {
		setRooms,
		roomsRef,
		activeRoomId,
		setActiveRoomId,
		activeRoomIdRef,
		opencodePorts,
		setOpencodePorts,
		loaded,
		loadedRef,
		loadFailed,
		setLoadFailed,
		quarantinedCount,
		setQuarantinedCount,
		backupRoomCount,
		setBackupRoomCount,
		missingFolders,
		checkRoomFolder,
		hydrateRooms,
		activeRooms,
		archivedRooms,
		archivedRoomsRef,
		room,
		activeHarness,
		unarchiveRoom,
		unarchiveRoomRef,
		recreateMissingWorktree,
		pickMissingFolder,
		deleteRoomForever,
		restoreRoom,
		closeRoom,
		switchRoom,
	} = useRoomsStore(defaultShell, setRenaming, lastUsedByGroupRef);

	// Keyboard nav (Mod+Tab, Mod+1..9) keys off active rooms only —
	// archived ones aren't rendered as tabs and shouldn't be reachable
	// via the cycle / jump shortcuts.
	const activeRoomsRef = useRef(activeRooms);
	activeRoomsRef.current = activeRooms;
	// #331: keep in sync for the status-popover breakdown — see
	// popoverRoomsRef's declaration above useAppWindowEffects.
	popoverRoomsRef.current = roomsRef.current;

	// #19: room-strip navigation (drag-reorder, the two-level strip's
	// segments, group "last used" memory, top-level tab click
	// resolution) and New Room opening + its persisted per-folder
	// memory, extracted to keep this file's growth minimal — see
	// useRoomStripNav.ts. Called here, right after useRoomsStore
	// (earlier than this code used to sit): `reorderRoom`/
	// `reorderHarness` feed `useTabDrag` below, and `setShowNewRoom`
	// feeds `useHarnessCreation` just below.
	const {
		reorderRoom,
		reorderHarness,
		stripSegments,
		stripSegmentsRef,
		activeSegment,
		onSelectSegment,
		showNewRoom,
		setShowNewRoom,
		newRoomMemory,
		newRoomSeed,
		openNewRoom,
		openNewRoomAt,
		openGroupPlaceholder,
		rememberRoomFolder,
		recentRoomFolders,
	} = useRoomStripNav(
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
	);
	// #76: derived from `stripSegments` above — kept here (not in the
	// hook) since keyboard nav is the only consumer and it already
	// lives in App.tsx.
	const visibleOrderRooms = useMemo(() => allRoomOrder(stripSegments), [stripSegments]);
	const visibleOrderRef = useRef(visibleOrderRooms);
	visibleOrderRef.current = visibleOrderRooms;

	// #19: harness/room action helpers (rename trio, switchHarnessInRoom,
	// jumpToHarness, the alerted-room/-harness cyclers, closeHarness,
	// updateHarnessCmd, addHarness), extracted to keep this file's
	// growth minimal — see useHarnessActions.ts. Called here, before
	// useHarnessCreation, which needs `switchHarnessInRoom`; none of the
	// rest here needs anything useHarnessCreation/useHarnessNotifications
	// return, so the whole set moved into this one call rather than
	// splitting across two.
	const {
		startRenameRoom,
		endRenameRoom,
		commitRenameRoom,
		switchHarnessInRoom,
		jumpToHarness,
		cycleAlertedRoom,
		cycleAlertedHarness,
		closeHarness,
		updateHarnessCmd,
		addHarness,
	} = useHarnessActions(
		setRooms,
		setActiveRoomId,
		activeRoomId,
		activeRooms,
		setShowPicker,
		setRenaming,
	);

	const reopenRoom = async (id: string) => {
		await unarchiveRoom(id);
		setShowReopen(false);
	};

	// #19: harness/room creation, extracted to keep this file's growth
	// minimal — see useHarnessCreation.ts.
	const {
		pickHarness,
		toggleFilesHarness,
		createRoom,
		setHarnessSessionId,
		replaceHarnessSessionId,
	} = useHarnessCreation(
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

	// #19: the notification engine (badge/toast/OS notifications,
	// harness-permission + harness-session-start listeners, transition
	// logging), extracted to keep this file's growth minimal — see
	// useHarnessNotifications.ts. Called here because it needs
	// `replaceHarnessSessionId` (useHarnessCreation, just above) and
	// `jumpToHarness` (useHarnessActions, earlier still);
	// `displayedHarnessId` is the (active room, active harness) tuple the
	// clear-pending-on-view effect watches — `room` comes from
	// useRoomsStore.
	const displayedHarnessId = room?.activeHarnessId ?? null;
	const { toasts, dismissToast, jumpToToast } = useHarnessNotifications(
		roomsRef,
		activeRoomIdRef,
		setRooms,
		activeRoomId,
		displayedHarnessId,
		notifyBadge,
		notifyToast,
		notifyOs,
		replaceHarnessSessionId,
		jumpToHarness,
	);

	// Pointer-based drag-to-reorder (#271, replacing the HTML5 DnD this
	// used before — see tabDrag.ts's header for why). The state machine
	// and its DOM wiring both live outside App.tsx; `reorderRoom` /
	// `reorderHarness` above are the only pieces the hook needs back.
	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = useTabDrag(
		reorderRoom,
		reorderHarness,
	);

	// Window-level keyboard shortcuts. Uses isAppShortcut as the gate —
	// that same predicate also makes LiveTerminal's xterm custom handler
	// return false for these combos, so the byte never reaches the PTY.
	// preventDefault stops the WebView's defaults (Mod+W close, Mod+=
	// zoom, Mod+1..9 tab jump, etc). Mod = ⌘ on macOS, Ctrl elsewhere.
	//
	// Stash the per-render handler refs so the listener can stay
	// bound across renders without re-listing every callback as a dep.
	const addHarnessRef = useRef(addHarness);
	addHarnessRef.current = addHarness;
	const closeRoomRef = useRef(closeRoom);
	closeRoomRef.current = closeRoom;
	const switchHarnessInRoomRef = useRef(switchHarnessInRoom);
	switchHarnessInRoomRef.current = switchHarnessInRoom;
	const cycleAlertedRoomRef = useRef(cycleAlertedRoom);
	cycleAlertedRoomRef.current = cycleAlertedRoom;
	const cycleAlertedHarnessRef = useRef(cycleAlertedHarness);
	cycleAlertedHarnessRef.current = cycleAlertedHarness;
	// toggleFilesHarness comes from useHarnessCreation (#19); the ref is
	// declared here with the others since other call sites read it
	// through the ref, not the hook's return value directly.
	const toggleFilesRef = useRef<() => void>(() => {});
	toggleFilesRef.current = toggleFilesHarness;
	// Mod+R (#212). Unlike Mod+E this needs nothing created — the review
	// pane is always mounted — so it is assigned here rather than later.
	// Reassigned every render so it closes over the current active room.
	const toggleReviewRef = useRef<() => void>(() => {});
	toggleReviewRef.current = () => {
		if (!activeRoomId) return;
		setRightPaneTab(
			activeRoomId,
			(rightPaneTabs[activeRoomId] ?? "context") === "review" ? "context" : "review",
		);
	};
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

	// #19: the notification engine (window-focus/permission refs, the
	// harness-permission + harness-session-start listeners, the
	// badge/toast/OS-notification transition subscriber, the api_error
	// toast effect, the db_record_harness_event log, and clear-pending-
	// on-view) now lives in useHarnessNotifications — see the hook call
	// above, right after useHarnessCreation (which it needs for
	// `replaceHarnessSessionId`; `jumpToHarness` itself now comes from
	// useHarnessActions, earlier still).

	// #19: the window-level keyboard shortcut listener (cycleRoom,
	// cycleHarness, and the Mod+… switch), extracted to keep this
	// file's growth minimal — see useKeyboardShortcuts.ts. Called here,
	// after every ref above has its `.current` mirror assigned and
	// `openNewRoom` is declared.
	useKeyboardShortcuts(
		visibleOrderRef,
		stripSegmentsRef,
		activeRoomIdRef,
		activeRoomsRef,
		switchHarnessInRoomRef,
		addHarnessRef,
		closeRoomRef,
		cycleAlertedRoomRef,
		cycleAlertedHarnessRef,
		toggleFilesRef,
		toggleReviewRef,
		lastUsedByGroupRef,
		setActiveRoomId,
		setShowPalette,
		setShowSettings,
		setFontSize,
		openNewRoom,
	);

	// #19: OS-notification click handling (jumpToTarget,
	// drainPendingClick, the live click listener, and the post-hydrate
	// drain), extracted to keep this file's growth minimal — see
	// useOsNotificationClicks.ts. NOT here: `jumpToToast`, which
	// already lives in useHarnessNotifications.
	useOsNotificationClicks(roomsRef, unarchiveRoomRef, setRooms, loaded, loadedRef);

	const titlebarProps: TitlebarProps = {
		activeRoomLabel: room ? room.name : null,
		onOpenSettings: () => setShowSettings(true),
	};

	const settingsProps = {
		theme,
		density,
		fontSize,
		fontMin: FONT_MIN,
		fontMax: FONT_MAX,
		chromeFontSize: chromeFontPt,
		chromeFontMin: CHROME_FONT_MIN,
		chromeFontMax: CHROME_FONT_MAX,
		copyOnSelect,
		onCopyOnSelect: setCopyOnSelect,
		onTheme: setTheme,
		onDensity: setDensity,
		onFontSize: setFontSize,
		onChromeFontSize: setChromeFontPt,
		notifyBadge,
		notifyToast,
		notifyUrgent,
		notifyOs,
		onNotifyBadge: setNotifyBadge,
		onNotifyToast: setNotifyToast,
		onNotifyUrgent: setNotifyUrgent,
		onNotifyOs: setNotifyOs,
		spawnSettings: spawnEnv?.settings ?? null,
		spawnDegraded: spawnEnv?.degraded ?? null,
		spawnSettingsPath: spawnEnv?.settingsPath ?? "",
		onSpawnSettings: saveSpawnSettings,
		defaultAgents,
		onDefaultAgent: (kind: HarnessKind, agent: string | undefined) =>
			setDefaultAgents((prev) => withDefaultAgent(prev, kind, agent)),
		branchTemplate,
		onBranchTemplate: setBranchTemplate,
		agentCwd: room?.cwd ?? defaultCwd,
		onClose: () => setShowSettings(false),
	};

	// Phase 4 / #19: items the command palette offers, built out to
	// paletteItems.ts (buildPaletteItems) to keep this file's growth
	// minimal. Still called as a plain function every render, not
	// useMemo'd — see that module's header for why.
	const paletteItems: PaletteItem[] = buildPaletteItems({
		activeRooms,
		archivedRooms,
		room,
		activeRoomId,
		theme,
		setActiveRoomId,
		setRooms,
		setTheme,
		setShowReopen,
		openNewRoom,
		toggleFilesRef,
		toggleReviewRef,
		addHarness,
		startRenameRoom,
		closeRoom,
		cycleAlertedRoom,
		cycleAlertedHarness,
	});

	// Empty state — no *active* rooms. Archived rooms still in the list
	// show via the reopen modal (linked from the empty state too).
	// Pre-hydration: rooms haven't loaded from sqlite yet, so `activeRooms`
	// is transiently []. Render a quiet shell (titlebar + blank pane), not
	// the EmptyState — otherwise users with existing rooms get a flash of
	// new-user onboarding every boot (#39). The auto-save effect is already
	// parked on !loaded, so this never writes over the unread DB.
	if (!loaded) {
		return (
			<div
				className={`sk-app sk-${theme} density-${density}`}
				data-platform={isMac ? "mac" : "other"}
				style={{ ["--cfs" as string]: `${chromeFontPt}px` }}
			>
				<Titlebar {...titlebarProps} />
				{loadFailed !== null ? (
					// #167: the load failed wholesale. The autosave is
					// parked (loaded stays false) so the DB is untouched;
					// offer retry instead of silently starting empty.
					<div className="sk-boot-error">
						<div className="sk-boot-error-card">
							<div className="sk-boot-error-title">Couldn't load your rooms</div>
							<div className="sk-boot-error-msg">{loadFailed}</div>
							<div className="sk-boot-error-hint">
								No saved rooms have been deleted (unreadable rows may have been moved to the
								sessions_quarantine table inside skein.db), and nothing will be saved until loading
								succeeds. Last-known-good snapshots sit next to it as skein.db.bak and .bak.1 — if
								you restore one manually, also delete skein.db-wal and skein.db-shm.
							</div>
							<div className="sk-boot-error-actions">
								<button
									type="button"
									className="sk-btn primary"
									onClick={() => {
										setLoadFailed(null);
										hydrateRooms();
									}}
								>
									Retry
								</button>
							</div>
						</div>
					</div>
				) : (
					<div className="sk-boot" />
				)}
			</div>
		);
	}

	if (activeRooms.length === 0) {
		return (
			<div
				className={`sk-app sk-${theme} density-${density}`}
				data-platform={isMac ? "mac" : "other"}
				style={{ ["--cfs" as string]: `${chromeFontPt}px` }}
			>
				<Titlebar {...titlebarProps} />
				<EmptyState
					onNew={() => void openNewRoom()}
					archivedCount={archivedRooms.length}
					onReopen={() => setShowReopen(true)}
				/>
				<AppOverlays
					showNewRoom={showNewRoom}
					defaultCwd={defaultCwd}
					newRoomSeed={newRoomSeed}
					defaultAgents={defaultAgents}
					newRoomMemory={newRoomMemory}
					branchTemplate={branchTemplate}
					recentRoomFolders={recentRoomFolders}
					rememberRoomFolder={rememberRoomFolder}
					createRoom={createRoom}
					setShowNewRoom={setShowNewRoom}
					showPalette={showPalette}
					paletteItems={paletteItems}
					setShowPalette={setShowPalette}
					showSettings={showSettings}
					settingsProps={settingsProps}
					showReopen={showReopen}
					archivedRooms={archivedRooms}
					reopenRoom={reopenRoom}
					deleteRoomForever={deleteRoomForever}
					restoreRoom={restoreRoom}
					setShowReopen={setShowReopen}
					toasts={toasts}
					jumpToToast={jumpToToast}
					dismissToast={dismissToast}
					quarantinedCount={quarantinedCount}
					setQuarantinedCount={setQuarantinedCount}
					backupRoomCount={backupRoomCount}
					setBackupRoomCount={setBackupRoomCount}
				/>
			</div>
		);
	}

	if (!room || !activeHarness) {
		// Shouldn't happen in practice: rooms is non-empty above.
		return null;
	}

	return (
		<div
			className={`sk-app sk-${theme} density-${density}`}
			data-platform={isMac ? "mac" : "other"}
			style={{ ["--cfs" as string]: `${chromeFontPt}px` }}
		>
			<Titlebar {...titlebarProps} />

			{/* #271: data-drag-strip lets useTabDrag's hitTest resolve a drop
			    over blank strip space or the `+` button to an end-of-strip
			    gap, instead of finding nothing draggable there. */}
			<div className="sk-tabstrip" data-drag-strip="room">
				<RoomStrip
					segments={stripSegments}
					activeRoomId={activeRoomId}
					onSelectSegment={onSelectSegment}
					onCloseRoom={(id) => void closeRoom(id)}
					dragWiring={{ drag, dropTarget, startDrag, dragHandlers, suppressClick }}
					renaming={renaming}
					onStartRename={startRenameRoom}
					onRename={commitRenameRoom}
					onRenameEnd={endRenameRoom}
				/>
				<div className="sk-tab-newbtn" onClick={() => void openNewRoom()} title="New room">
					+
				</div>
			</div>
			{/* #76: the second row — the active group's own rooms, main
			    pinned first — only when the active room is IN a group. A
			    repository with a single open room stays a plain top-level
			    tab and never grows this row. */}
			{activeSegment?.kind === "group" && (
				<GroupRow
					seg={activeSegment}
					activeRoomId={activeRoomId}
					onSwitchRoom={switchRoom}
					onCloseRoom={(id) => void closeRoom(id)}
					onOpenPlaceholder={openGroupPlaceholder}
					onNewRoom={openNewRoomAt}
					dragWiring={{ drag, dropTarget, startDrag, dragHandlers, suppressClick }}
					renaming={renaming}
					onStartRename={startRenameRoom}
					onRename={commitRenameRoom}
					onRenameEnd={endRenameRoom}
				/>
			)}

			<Splitter
				className="sk-workspace"
				direction="row"
				size={harnessColWidth}
				onResize={setHarnessColWidth}
				minFirst={320}
				minSecond={320}
				first={activeRooms.map((r) => (
					<div
						key={r.id}
						style={{
							display: r.id === activeRoomId ? "flex" : "none",
							flexDirection: "column",
							flex: 1,
							minHeight: 0,
						}}
					>
						{missingFolders.has(r.id) ? (
							// #164: no HarnessColumn mounts here, so no LiveTerminal
							// spawns anywhere — a missing folder never silently runs
							// its harnesses somewhere else.
							<MissingFolderCard
								room={r}
								onRecreateWorktree={() => recreateMissingWorktree(r)}
								onPickFolder={(newCwd) => void pickMissingFolder(r, newCwd)}
								onClose={() => void closeRoom(r.id)}
							/>
						) : (
							<HarnessColumn
								room={r}
								fontSize={fontSize}
								copyOnSelect={copyOnSelect}
								defaultShell={defaultShell}
								showPicker={showPicker === r.id}
								roomActive={r.id === activeRoomId}
								harnessDrag={{
									draggedHarnessId:
										drag?.kind === "harness" && drag.roomId === r.id ? drag.id : null,
									dropTargetHarnessId:
										dropTarget?.kind === "harness" && dropTarget.roomId === r.id
											? dropTarget.id
											: null,
									dropSide:
										dropTarget?.kind === "harness" && dropTarget.roomId === r.id
											? dropTarget.side
											: null,
									onPointerDown: (e, roomId, harnessId) =>
										startDrag(e, { kind: "harness", roomId, id: harnessId }),
									onPointerMove: dragHandlers.onPointerMove,
									onPointerUp: dragHandlers.onPointerUp,
									onPointerCancel: dragHandlers.onPointerCancel,
									onLostPointerCapture: dragHandlers.onLostPointerCapture,
									suppressClick,
								}}
								defaultAgents={defaultAgents}
								onPick={pickHarness}
								onAddHarness={addHarness}
								onCancelPick={() => setShowPicker(null)}
								onSwitchHarness={switchHarnessInRoom}
								onCloseHarness={closeHarness}
								onHarnessCmdChange={updateHarnessCmd}
								opencodePorts={opencodePorts}
								onOpencodeSessionCaptured={(harnessId, sid) =>
									setHarnessSessionId(r.id, harnessId, sid)
								}
								onOpencodeSessionFollowed={(harnessId, sid) =>
									replaceHarnessSessionId(r.id, harnessId, sid)
								}
							/>
						)}
					</div>
				))}
				second={activeRooms.map((r) => (
					<div
						key={r.id}
						className="sk-right"
						style={{
							display: r.id === activeRoomId ? "flex" : "none",
						}}
					>
						{/* The right pane is two tabs (#212): Live Context — the
						    Plan / Activity card stack sourced from harness_actions
						    (issue #80) — and Review, the branch-vs-base diff and
						    comment surface that replaced the Diff card. Live
						    Context also keeps the status-bar branch fresh via a
						    lightweight git watcher (issue #18), the role LiveStatus
						    used to own. */}
						{r.cwd ? (
							<RightPane
								roomId={r.id}
								cwd={r.cwd}
								harnesses={r.harnesses}
								activeHarness={r.harnesses.find((h) => h.id === r.activeHarnessId)}
								visible={r.id === activeRoomId}
								showTurnCosts={showTurnCosts}
								onToggleTurnCosts={handleToggleTurnCosts}
								onBranchChange={handleBranchChange}
								tab={rightPaneTabs[r.id] ?? "context"}
								onTabChange={(tab) => setRightPaneTab(r.id, tab)}
							/>
						) : null}
					</div>
				))}
			/>

			<StatusBar
				activeHarness={activeHarness}
				room={room}
				liveBranches={liveBranches}
				notifyUrgent={notifyUrgent}
				activeRooms={activeRooms}
				activeRoomId={activeRoomId}
				permissionHarnessIds={permissionHarnessIds}
				jumpToHarness={jumpToHarness}
				switchRoom={switchRoom}
			/>

			<AppOverlays
				showNewRoom={showNewRoom}
				defaultCwd={defaultCwd}
				newRoomSeed={newRoomSeed}
				defaultAgents={defaultAgents}
				newRoomMemory={newRoomMemory}
				branchTemplate={branchTemplate}
				recentRoomFolders={recentRoomFolders}
				rememberRoomFolder={rememberRoomFolder}
				createRoom={createRoom}
				setShowNewRoom={setShowNewRoom}
				showPalette={showPalette}
				paletteItems={paletteItems}
				setShowPalette={setShowPalette}
				showSettings={showSettings}
				settingsProps={settingsProps}
				showReopen={showReopen}
				archivedRooms={archivedRooms}
				reopenRoom={reopenRoom}
				deleteRoomForever={deleteRoomForever}
				restoreRoom={restoreRoom}
				setShowReopen={setShowReopen}
				toasts={toasts}
				jumpToToast={jumpToToast}
				dismissToast={dismissToast}
				quarantinedCount={quarantinedCount}
				setQuarantinedCount={setQuarantinedCount}
				backupRoomCount={backupRoomCount}
				setBackupRoomCount={setBackupRoomCount}
			/>
		</div>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
