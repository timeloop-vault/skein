import { useLayoutEffect, useMemo, useRef } from "react";
import { publishDesignFocus } from "./designFocus.ts";
import { LeftSlot, RightSlot, type RoomHandlers } from "./RoomWorkspaceSlots.tsx";
import { Splitter } from "./Splitter.tsx";
import type { Room } from "./types.ts";
import type { useAppSettings } from "./useAppSettings.ts";
import type { useHarnessActions } from "./useHarnessActions.ts";
import type { useHarnessCreation } from "./useHarnessCreation.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";
import type { useTabDrag } from "./useTabDrag.ts";
import "./RoomWorkspace.css";

type Store = ReturnType<typeof useRoomsStore>;
type Settings = ReturnType<typeof useAppSettings>;
type Actions = ReturnType<typeof useHarnessActions>;
type Creation = ReturnType<typeof useHarnessCreation>;
type TabDrag = Omit<ReturnType<typeof useTabDrag>, "refused">;

export interface RoomWorkspaceProps {
	activeRooms: Room[];
	activeRoomId: string;
	store: Store;
	actions: Actions;
	creation: Creation;
	settings: Settings;
	defaultShell: string[];
	showPicker: string | null;
	setShowPicker: (id: string | null) => void;
	drag: TabDrag;
	onReattachTelemetry: (roomId: string, harnessId: string) => void;
}

// #594: one handler bag, stable for the component's life. Hooks upstream
// return fresh closures every render, so each handler reads the latest props
// through a ref and takes the roomId explicitly.
const useRoomHandlers = (p: RoomWorkspaceProps): RoomHandlers => {
	// Handlers read props via a ref updated in useLayoutEffect: call them only from events or
	// passive effects, never from a child's layout effect in the same commit.
	const latest = useRef(p);
	useLayoutEffect(() => {
		latest.current = p;
	});
	return useMemo(
		(): RoomHandlers => ({
			pick: (kind, agent, remote) => latest.current.creation.pickHarness(kind, agent, remote),
			addHarness: (roomId) => latest.current.actions.addHarness(roomId),
			cancelPick: () => latest.current.setShowPicker(null),
			switchHarness: (roomId, harnessId) => latest.current.actions.selectHarness(roomId, harnessId),
			closeHarness: (roomId, harnessId) => latest.current.actions.closeHarness(roomId, harnessId),
			cmdChange: (roomId, harnessId, cmd) =>
				latest.current.actions.updateHarnessCmd(roomId, harnessId, cmd),
			designEntry: (roomId, harnessId, entry) =>
				latest.current.actions.setHarnessDesignEntry(roomId, harnessId, entry),
			designDevice: (roomId, harnessId, device) =>
				latest.current.actions.setHarnessDesignDevice(roomId, harnessId, device),
			designDock: (roomId, harnessId, docked) => {
				const { settings, actions } = latest.current;
				settings.setDesignDocked(harnessId, docked);
				// #551: reveal a docked design harness in its room's right pane.
				if (docked) settings.showDockedDesign(roomId, harnessId);
				else actions.switchHarnessInRoom(roomId, harnessId);
			},
			designShow: (roomId, harnessId) =>
				latest.current.settings.showDockedDesign(roomId, harnessId),
			designPick: (roomId, harnessId) => latest.current.settings.setDesignPick(roomId, harnessId),
			designUndock: (roomId, harnessId) => {
				latest.current.settings.setDesignDocked(harnessId, false);
				latest.current.actions.switchHarnessInRoom(roomId, harnessId);
			},
			sessionCaptured: (roomId, harnessId, sid) =>
				latest.current.creation.setHarnessSessionId(roomId, harnessId, sid),
			sessionFollowed: (roomId, harnessId, sid) =>
				latest.current.creation.replaceHarnessSessionId(roomId, harnessId, sid),
			shellClaimSet: (roomId, harnessId, sid, port) =>
				latest.current.creation.setHarnessShellClaim(roomId, harnessId, sid, port),
			shellClaimRelease: (roomId, harnessId) =>
				latest.current.creation.clearHarnessShellClaim(roomId, harnessId),
			reattach: (roomId, harnessId) => latest.current.onReattachTelemetry(roomId, harnessId),
			restart: (roomId, harnessId) => latest.current.actions.restartHarness(roomId, harnessId),
			recreateWorktree: (room) => latest.current.store.recreateMissingWorktree(room),
			pickFolder: (room, newCwd) => latest.current.store.pickMissingFolder(room, newCwd),
			closeRoom: (roomId) => latest.current.store.closeRoom(roomId),
			retire: (roomId) => latest.current.store.retireMismatchedRoom(roomId),
			sameRepo: (roomId) => latest.current.store.confirmSameRepo(roomId),
			toggleTurnCosts: () => latest.current.settings.handleToggleTurnCosts(),
			branchChange: (roomId, branch) => latest.current.settings.handleBranchChange(roomId, branch),
			tabChange: (roomId, tab) => latest.current.settings.setRightPaneTab(roomId, tab),
			showInDesign: (roomId, pick, entry, threadId) => {
				const { settings, actions } = latest.current;
				if (pick.setEntry) actions.setHarnessDesignEntry(roomId, pick.harnessId, entry);
				if (settings.dockedDesign[pick.harnessId])
					settings.showDockedDesign(roomId, pick.harnessId);
				else actions.switchHarnessInRoom(roomId, pick.harnessId);
				publishDesignFocus({ roomId, harnessId: pick.harnessId, entry, threadId });
			},
			dragStart: (e, roomId, harnessId) =>
				latest.current.drag.startDrag(e, { kind: "harness", roomId, id: harnessId }),
			dragMove: (e) => latest.current.drag.dragHandlers.onPointerMove(e),
			dragUp: (e) => latest.current.drag.dragHandlers.onPointerUp(e),
			dragCancel: (e) => latest.current.drag.dragHandlers.onPointerCancel(e),
			dragLost: (e) => latest.current.drag.dragHandlers.onLostPointerCapture(e),
			suppressClick: () => latest.current.drag.suppressClick(),
		}),
		[],
	);
};

// The two-pane workspace: every active room's harness column on the left
// and its right pane on the right, all kept mounted and hidden with
// display:none so PTYs survive room switches.
export const RoomWorkspace = (p: RoomWorkspaceProps) => {
	const { drag, dropTarget } = p.drag;
	const s = p.settings;
	const h = useRoomHandlers(p);
	return (
		<Splitter
			className="sk-workspace"
			direction="row"
			size={s.harnessColWidth}
			onResize={s.setHarnessColWidth}
			minFirst={320}
			minSecond={320}
			first={p.activeRooms.map((r) => {
				const dragHere = drag?.kind === "harness" && drag.roomId === r.id ? drag : null;
				const dropHere =
					dropTarget?.kind === "harness" && dropTarget.roomId === r.id ? dropTarget : null;
				return (
					<div
						key={r.id}
						style={{
							display: r.id === p.activeRoomId ? "flex" : "none",
							flexDirection: "column",
							flex: 1,
							minHeight: 0,
						}}
					>
						<LeftSlot
							room={r}
							active={r.id === p.activeRoomId}
							missing={p.store.missingFolders.has(r.id)}
							mismatched={p.store.mismatchedRooms.has(r.id)}
							showPicker={p.showPicker === r.id}
							fontSize={s.fontSize}
							copyOnSelect={s.copyOnSelect}
							defaultShell={p.defaultShell}
							defaultAgents={s.defaultAgents}
							dockedDesign={s.dockedDesign}
							opencodePorts={p.store.opencodePorts}
							versionNoticeModes={s.versionNoticeModes}
							draggedHarnessId={dragHere ? dragHere.id : null}
							dropTargetHarnessId={dropHere ? dropHere.id : null}
							dropSide={dropHere ? dropHere.side : null}
							h={h}
						/>
					</div>
				);
			})}
			second={p.activeRooms.map((r) => (
				<div
					key={r.id}
					className="sk-right"
					style={{
						display: r.id === p.activeRoomId ? "flex" : "none",
					}}
				>
					{/* The right pane is two tabs (#212): Live Context — the
					    Plan / Activity card stack sourced from harness_actions
					    (issue #80) — and Review, the branch-vs-base diff and
					    comment surface that replaced the Diff card. Live
					    Context also keeps the status-bar branch fresh via a
					    lightweight git watcher (issue #18), the role LiveStatus
					    used to own. */}
					<RightSlot
						room={r}
						active={r.id === p.activeRoomId}
						gated={p.store.missingFolders.has(r.id) || p.store.mismatchedRooms.has(r.id)}
						showTurnCosts={s.showTurnCosts}
						tab={s.rightPaneTabs[r.id] ?? "context"}
						dockedDesign={s.dockedDesign}
						designPick={s.designPicks[r.id]}
						h={h}
					/>
				</div>
			))}
		/>
	);
};
