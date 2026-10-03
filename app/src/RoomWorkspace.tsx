import { publishDesignFocus } from "./designFocus.ts";
import { HarnessColumn } from "./HarnessColumn.tsx";
import { MissingFolderCard } from "./MissingFolderCard.tsx";
import { RepoMismatchCard } from "./RepoMismatchCard.tsx";
import { RightPane } from "./RightPane.tsx";
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

// The two-pane workspace: every active room's harness column on the left
// and its right pane on the right, all kept mounted and hidden with
// display:none so PTYs survive room switches.
export const RoomWorkspace = (p: RoomWorkspaceProps) => {
	const { drag, dropTarget, startDrag, dragHandlers, suppressClick } = p.drag;
	const s = p.settings;
	return (
		<Splitter
			className="sk-workspace"
			direction="row"
			size={s.harnessColWidth}
			onResize={s.setHarnessColWidth}
			minFirst={320}
			minSecond={320}
			first={p.activeRooms.map((r) => (
				<div
					key={r.id}
					style={{
						display: r.id === p.activeRoomId ? "flex" : "none",
						flexDirection: "column",
						flex: 1,
						minHeight: 0,
					}}
				>
					{p.store.missingFolders.has(r.id) ? (
						// #164: no HarnessColumn mounts here, so no LiveTerminal
						// spawns anywhere — a missing folder never silently runs
						// its harnesses somewhere else.
						<MissingFolderCard
							room={r}
							onRecreateWorktree={() => p.store.recreateMissingWorktree(r)}
							onPickFolder={(newCwd) => void p.store.pickMissingFolder(r, newCwd)}
							onClose={() => void p.store.closeRoom(r.id)}
						/>
					) : p.store.mismatchedRooms.has(r.id) ? (
						// #418: same gate — a different repository now lives here,
						// so nothing resumes until the user decides.
						<RepoMismatchCard
							room={r}
							onRetire={() => p.store.retireMismatchedRoom(r.id)}
							onRepoint={(newCwd) => void p.store.pickMissingFolder(r, newCwd)}
							onSameRepo={() => void p.store.confirmSameRepo(r.id)}
						/>
					) : (
						<HarnessColumn
							room={r}
							fontSize={s.fontSize}
							copyOnSelect={s.copyOnSelect}
							defaultShell={p.defaultShell}
							showPicker={p.showPicker === r.id}
							roomActive={r.id === p.activeRoomId}
							harnessDrag={{
								draggedHarnessId: drag?.kind === "harness" && drag.roomId === r.id ? drag.id : null,
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
							defaultAgents={s.defaultAgents}
							onPick={p.creation.pickHarness}
							onAddHarness={p.actions.addHarness}
							onCancelPick={() => p.setShowPicker(null)}
							onSwitchHarness={p.actions.switchHarnessInRoom}
							onCloseHarness={p.actions.closeHarness}
							onHarnessCmdChange={p.actions.updateHarnessCmd}
							onDesignEntryChange={p.actions.setHarnessDesignEntry}
							opencodePorts={p.store.opencodePorts}
							onOpencodeSessionCaptured={(harnessId, sid) =>
								p.creation.setHarnessSessionId(r.id, harnessId, sid)
							}
							onOpencodeSessionFollowed={(harnessId, sid) =>
								p.creation.replaceHarnessSessionId(r.id, harnessId, sid)
							}
							onReattachTelemetry={(harnessId) => p.onReattachTelemetry(r.id, harnessId)}
							versionNoticeModes={s.versionNoticeModes}
							onRestartHarness={(harnessId) => p.actions.restartHarness(r.id, harnessId)}
						/>
					)}
				</div>
			))}
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
					{r.cwd ? (
						<RightPane
							roomId={r.id}
							cwd={r.cwd}
							harnesses={r.harnesses}
							activeHarness={r.harnesses.find((h) => h.id === r.activeHarnessId)}
							visible={r.id === p.activeRoomId}
							gated={p.store.missingFolders.has(r.id) || p.store.mismatchedRooms.has(r.id)}
							showTurnCosts={s.showTurnCosts}
							onToggleTurnCosts={s.handleToggleTurnCosts}
							onBranchChange={s.handleBranchChange}
							tab={s.rightPaneTabs[r.id] ?? "context"}
							onTabChange={(tab) => s.setRightPaneTab(r.id, tab)}
							onShowInDesign={(pick, entry, threadId) => {
								if (pick.setEntry) p.actions.setHarnessDesignEntry(r.id, pick.harnessId, entry);
								p.actions.switchHarnessInRoom(r.id, pick.harnessId);
								publishDesignFocus({ roomId: r.id, harnessId: pick.harnessId, entry, threadId });
							}}
						/>
					) : null}
				</div>
			))}
		/>
	);
};
