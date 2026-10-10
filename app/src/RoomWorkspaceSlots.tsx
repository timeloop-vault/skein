import { memo, type PointerEvent as ReactPointerEvent, useCallback, useMemo } from "react";
import type { DesignDevice } from "./designDevice.ts";
import { type DockedDesign, dockedDesignHarnesses } from "./designDock.ts";
import { HarnessColumn } from "./HarnessColumn.tsx";
import type { GateResult } from "./harnessInputGate.ts";
import { MissingFolderCard } from "./MissingFolderCard.tsx";
import type { DefaultAgents, VersionNoticeModes } from "./prefs.ts";
import { RepoMismatchCard } from "./RepoMismatchCard.tsx";
import { RightPane, type RightPaneTab } from "./RightPane.tsx";
import type { Room } from "./types.ts";
import type { useHarnessCreation } from "./useHarnessCreation.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";

type Store = ReturnType<typeof useRoomsStore>;
type Creation = ReturnType<typeof useHarnessCreation>;
type PointerEv = ReactPointerEvent<HTMLDivElement>;

// #594: one stable bag of handlers for every room's slots. Each takes the
// roomId explicitly and delegates through RoomWorkspace's latest-props ref, so
// its identity never changes and an unrelated state change in one room does not
// re-render the other rooms' columns and panes.
export interface RoomHandlers {
	pick: Creation["pickHarness"];
	addHarness: (roomId: string) => void;
	cancelPick: () => void;
	switchHarness: (roomId: string, harnessId: string) => void;
	closeHarness: (roomId: string, harnessId: string) => void;
	cmdChange: (roomId: string, harnessId: string, cmd: string[]) => void;
	designEntry: (roomId: string, harnessId: string, entry: string) => void;
	designDevice: (roomId: string, harnessId: string, device: DesignDevice | undefined) => void;
	designDock: (roomId: string, harnessId: string, docked: boolean) => void;
	designShow: (roomId: string, harnessId: string) => void;
	designPick: (roomId: string, harnessId: string) => void;
	designUndock: (roomId: string, harnessId: string) => void;
	sessionCaptured: (roomId: string, harnessId: string, sessionId: string) => void;
	sessionFollowed: (roomId: string, harnessId: string, sessionId: string) => void;
	shellClaimSet: (roomId: string, harnessId: string, sessionId: string, port?: number) => void;
	shellClaimRelease: (roomId: string, harnessId: string) => void;
	reattach: (roomId: string, harnessId: string) => void;
	restart: (roomId: string, harnessId: string) => Promise<GateResult>;
	recreateWorktree: Store["recreateMissingWorktree"];
	pickFolder: Store["pickMissingFolder"];
	closeRoom: Store["closeRoom"];
	retire: Store["retireMismatchedRoom"];
	sameRepo: Store["confirmSameRepo"];
	toggleTurnCosts: () => void;
	branchChange: (roomId: string, branch: string | null) => void;
	tabChange: (roomId: string, tab: RightPaneTab) => void;
	showInDesign: (
		roomId: string,
		pick: { harnessId: string; setEntry: boolean },
		entry: string,
		threadId: string,
	) => void;
	dragStart: (e: PointerEv, roomId: string, harnessId: string) => void;
	dragMove: (e: PointerEv) => void;
	dragUp: (e: PointerEv) => void;
	dragCancel: (e: PointerEv) => void;
	dragLost: (e: PointerEv) => void;
	suppressClick: () => boolean;
}

export interface LeftSlotProps {
	room: Room;
	active: boolean;
	missing: boolean;
	mismatched: boolean;
	showPicker: boolean;
	fontSize: number;
	copyOnSelect: boolean;
	defaultShell: string[];
	defaultAgents: DefaultAgents;
	dockedDesign: DockedDesign;
	opencodePorts: Map<string, number>;
	versionNoticeModes: VersionNoticeModes;
	draggedHarnessId: string | null;
	dropTargetHarnessId: string | null;
	dropSide: "before" | "after" | null;
	h: RoomHandlers;
}

// The left slot of one room: the recovery card or the harness column.
export const LeftSlot = memo((p: LeftSlotProps) => {
	const { room, h } = p;
	const id = room.id;
	const harnessDrag = useMemo(
		() => ({
			draggedHarnessId: p.draggedHarnessId,
			dropTargetHarnessId: p.dropTargetHarnessId,
			dropSide: p.dropSide,
			onPointerDown: h.dragStart,
			onPointerMove: h.dragMove,
			onPointerUp: h.dragUp,
			onPointerCancel: h.dragCancel,
			onLostPointerCapture: h.dragLost,
			suppressClick: h.suppressClick,
		}),
		[p.draggedHarnessId, p.dropTargetHarnessId, p.dropSide, h],
	);
	const onDesignDock = useCallback(
		(harnessId: string, docked: boolean) => h.designDock(id, harnessId, docked),
		[h, id],
	);
	const onDesignShow = useCallback((harnessId: string) => h.designShow(id, harnessId), [h, id]);
	const onOpencodeSessionCaptured = useCallback(
		(harnessId: string, sid: string) => h.sessionCaptured(id, harnessId, sid),
		[h, id],
	);
	const onOpencodeSessionFollowed = useCallback(
		(harnessId: string, sid: string) => h.sessionFollowed(id, harnessId, sid),
		[h, id],
	);
	const opencodeShellClaim = useCallback(
		(harnessId: string) => ({
			set: (sid: string, port?: number) => h.shellClaimSet(id, harnessId, sid, port),
			release: () => h.shellClaimRelease(id, harnessId),
		}),
		[h, id],
	);
	const onReattachTelemetry = useCallback(
		(harnessId: string) => h.reattach(id, harnessId),
		[h, id],
	);
	const onRestartHarness = useCallback((harnessId: string) => h.restart(id, harnessId), [h, id]);

	if (p.missing) {
		// #164: no HarnessColumn mounts here, so no LiveTerminal
		// spawns anywhere — a missing folder never silently runs
		// its harnesses somewhere else.
		return (
			<MissingFolderCard
				room={room}
				onRecreateWorktree={() => h.recreateWorktree(room)}
				onPickFolder={(newCwd) => void h.pickFolder(room, newCwd)}
				onClose={() => void h.closeRoom(id)}
			/>
		);
	}
	if (p.mismatched) {
		// #418: same gate — a different repository now lives here,
		// so nothing resumes until the user decides.
		return (
			<RepoMismatchCard
				room={room}
				onRetire={() => h.retire(id)}
				onRepoint={(newCwd) => void h.pickFolder(room, newCwd)}
				onSameRepo={() => void h.sameRepo(id)}
			/>
		);
	}
	return (
		<HarnessColumn
			room={room}
			fontSize={p.fontSize}
			copyOnSelect={p.copyOnSelect}
			defaultShell={p.defaultShell}
			showPicker={p.showPicker}
			roomActive={p.active}
			harnessDrag={harnessDrag}
			defaultAgents={p.defaultAgents}
			onPick={h.pick}
			onAddHarness={h.addHarness}
			onCancelPick={h.cancelPick}
			onSwitchHarness={h.switchHarness}
			onCloseHarness={h.closeHarness}
			onHarnessCmdChange={h.cmdChange}
			onDesignEntryChange={h.designEntry}
			onDesignDeviceChange={h.designDevice}
			dockedDesign={p.dockedDesign}
			onDesignDock={onDesignDock}
			onDesignShow={onDesignShow}
			opencodePorts={p.opencodePorts}
			onOpencodeSessionCaptured={onOpencodeSessionCaptured}
			onOpencodeSessionFollowed={onOpencodeSessionFollowed}
			opencodeShellClaim={opencodeShellClaim}
			onReattachTelemetry={onReattachTelemetry}
			versionNoticeModes={p.versionNoticeModes}
			onRestartHarness={onRestartHarness}
		/>
	);
});

export interface RightSlotProps {
	room: Room;
	active: boolean;
	gated: boolean;
	showTurnCosts: boolean;
	tab: RightPaneTab;
	dockedDesign: DockedDesign;
	designPick: string | undefined;
	h: RoomHandlers;
}

// The right slot of one room: its RightPane (Live Context / Review / Design).
export const RightSlot = memo((p: RightSlotProps) => {
	const { room: r, h } = p;
	const id = r.id;
	const activeHarness = useMemo(
		() => r.harnesses.find((x) => x.id === r.activeHarnessId),
		[r.harnesses, r.activeHarnessId],
	);
	const docked = useMemo(
		() => dockedDesignHarnesses(r.harnesses, p.dockedDesign),
		[r.harnesses, p.dockedDesign],
	);
	const onBranchChange = h.branchChange;
	const design = useMemo(
		() => ({
			room: r,
			docked,
			pick: p.designPick,
			onPick: (harnessId: string) => h.designPick(id, harnessId),
			onEntryChange: h.designEntry,
			onDeviceChange: h.designDevice,
			onUndock: (harnessId: string) => h.designUndock(id, harnessId),
		}),
		[r, docked, p.designPick, h, id],
	);
	const onTabChange = useCallback((tab: RightPaneTab) => h.tabChange(id, tab), [h, id]);
	const onShowInDesign = useCallback(
		(pick: { harnessId: string; setEntry: boolean }, entry: string, threadId: string) =>
			h.showInDesign(id, pick, entry, threadId),
		[h, id],
	);
	if (!r.cwd) return null;
	return (
		<RightPane
			roomId={id}
			cwd={r.cwd}
			harnesses={r.harnesses}
			activeHarness={activeHarness}
			visible={p.active}
			gated={p.gated}
			showTurnCosts={p.showTurnCosts}
			onToggleTurnCosts={h.toggleTurnCosts}
			onBranchChange={onBranchChange}
			design={design}
			tab={p.tab}
			onTabChange={onTabChange}
			onShowInDesign={onShowInDesign}
		/>
	);
});
