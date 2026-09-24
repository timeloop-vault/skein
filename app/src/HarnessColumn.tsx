import { type PointerEvent as ReactPointerEvent, useEffect, useRef } from "react";
import { FilesBody } from "./FilesBody.tsx";
import { HarnessActionsMenu } from "./HarnessActionsMenu.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import { HarnessPicker, HarnessTab } from "./components.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import {
	activityToStatus,
	effectiveStatus,
	statusLabel,
	useHarnessActivity,
} from "./harnessActivity.ts";
import { agentLabel, useObservedAgent } from "./harnessAgent.ts";
import type { DefaultAgents } from "./prefs.ts";
import { useWorkingSubagentCount } from "./subagents.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";

// ── Live wrappers that subscribe to the activity store ─────────────
//
// HarnessTab, RoomTab, and the status bar all need to reflect what
// each harness/room is *actually* doing right now (running / idle
// / exited) rather than the hard-coded "running" stamped at
// creation time. Each instance subscribes via the activity hooks;
// they only re-render on real phase changes so a harness streaming
// output continuously doesn't churn its tab. Epic #50 (foundation
// for #29, #12, etc.).

const LiveHarnessTab = (props: Parameters<typeof HarnessTab>[0]) => {
	const activity = useHarnessActivity(props.h.id);
	const agent = agentLabel(props.h, useObservedAgent(props.h.id));
	if (!activity) return <HarnessTab {...props} agent={agent} />;
	// Apply the acknowledged-downgrade: a waiting harness with no
	// pending notifications has already been seen, so render it as
	// idle (grey) instead of waiting (blue pulse). The phase in
	// the store stays `waiting` — only the visual indicator
	// collapses.
	const status = effectiveStatus(activity, props.h.pendingNotifications ?? 0);
	return <HarnessTab {...props} agent={agent} h={{ ...props.h, status }} />;
};

// L4/L5a — per-room aggregate status + derived badge now live in
// RoomStrip.tsx (#76): `LiveRoomTab` for one room, `GroupTab` for a
// repo group's top-level tab, which folds over all its rooms' harnesses.

export const LiveStatusBarChip = ({ harness }: { harness: Harness }) => {
	const activity = useHarnessActivity(harness.id);
	// #277: reactively needed so "delegating · N agents" updates live
	// as subagents start/finish, not just on the next unrelated
	// harnessActivity emit.
	const workingCount = useWorkingSubagentCount(harness.id);
	// Dot color uses effectiveStatus so a waiting-but-acknowledged
	// harness renders grey (no pulse) in the bottom bar. The TEXT
	// keeps the underlying phase via activityToStatus — telling the
	// user "idle" when Claude is sitting at a prompt would be a lie;
	// the visual collapse to grey is a UX choice, the text isn't.
	const dotStatus = activity
		? effectiveStatus(activity, harness.pendingNotifications ?? 0)
		: harness.status;
	// #86: "permission needed" (+ tool) rather than the bare word.
	// #277: "delegating · N agents" in place of bare "running" while
	// subagents are working.
	const label = activity
		? statusLabel(
				activityToStatus(activity),
				activity.permissionTool,
				activity.permissionAgentType,
				workingCount,
			)
		: harness.status;
	return (
		<span className="seg">
			<span className={`dot-tiny st-${dotStatus}`} />
			{label}
		</span>
	);
};

// #248: which agent the active harness is on, worded by `agentLabel` so
// an opencode harness says "started as" rather than claiming to know.
export const AgentStatusBarSeg = ({ harness }: { harness: Harness }) => {
	const label = agentLabel(harness, useObservedAgent(harness.id));
	if (!label) return null;
	return (
		<span className="seg sk-statusbar-agent" title={label.title}>
			<span className="pk">{label.key}</span>
			{label.value}
		</span>
	);
};

// ── Harness body ───────────────────────────────────────────────────

interface HarnessBodyProps {
	harness: Harness;
	fontSize: number;
	// #158: copy-on-select setting — read live via a ref in LiveTerminal,
	// so toggling it applies to an already-running terminal without a
	// respawn. Threaded down exactly like fontSize.
	copyOnSelect: boolean;
	defaultShell: string[];
	visible: boolean;
	onCmdChange: (cmd: string[]) => void;
	// Stamped on every `harness_actions` row this harness emits
	// (issue #80). Threaded down to LiveTerminal → attachClaudeEvents.
	roomId: string;
	// Epic #50 L2c-2: opencode embedded-server port allocated by App.
	// `undefined` for non-opencode harnesses and for opencode harnesses
	// where pick_free_port failed — in the latter case the adapter
	// can't attach and the harness falls back to L2a.
	opencodePort: number | undefined;
	// SSE-capture callback: L2c-2 captures opencode's auto-allocated
	// sessionID from the `session.created` event. App.tsx wires it
	// to setHarnessSessionId; `undefined` for non-opencode harnesses.
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	// #116: L2c-2 decided the harness followed its TUI onto a different
	// root session (`/new` or a `/sessions` pick). App.tsx wires it to
	// replaceHarnessSessionId, same as Claude's clear/resume/fork
	// follow; `undefined` for non-opencode harnesses.
	onSessionFollowed: ((sessionId: string) => void) | undefined;
}

const HarnessBody = ({
	harness,
	fontSize,
	copyOnSelect,
	defaultShell,
	visible,
	onCmdChange,
	roomId,
	opencodePort,
	onSessionCaptured,
	onSessionFollowed,
}: HarnessBodyProps) => {
	if (harness.cmd && harness.cwd !== undefined) {
		// mountKey changes on cmd content OR spawnGen — the trigger for a
		// clean unmount + remount when the user picks Enter-for-shell
		// after a child exits. spawnGen is the explicit respawn signal:
		// the cmd-identity part alone misses a shell→shell respawn (new
		// cmd == old cmd → no remount → #53). Joining the array gives a
		// value-equal string across content-identical renders, so a
		// re-render with the same cmd + spawnGen doesn't churn the PTY.
		return (
			<LiveTerminal
				cmd={harness.cmd}
				cwd={harness.cwd}
				mountKey={`${harness.id}:${harness.spawnGen ?? 0}:${harness.cmd.join("\x00")}`}
				harnessId={harness.id}
				roomId={roomId}
				harnessKind={harness.kind}
				sessionId={harness.sessionId}
				agent={harness.agent}
				opencodePort={opencodePort}
				onSessionCaptured={onSessionCaptured}
				onSessionFollowed={onSessionFollowed}
				fontSize={fontSize}
				copyOnSelect={copyOnSelect}
				defaultShell={defaultShell}
				visible={visible}
				onCmdChange={onCmdChange}
			/>
		);
	}
	return null;
};

// ── Harness column (per room) ──────────────────────────────────────
// Every room's column stays mounted at once; the App-level renderer
// toggles visibility with display:none. PTYs survive tab switches
// because LiveTerminal's effect is keyed on mountKey only.

export interface HarnessDrag {
	draggedHarnessId: string | null;
	dropTargetHarnessId: string | null;
	dropSide: "before" | "after" | null;
	// #271: pointer-based drag start, scoped to this room by the caller
	// (App.tsx curries roomId in); the rest — move/up/cancel — doesn't
	// need per-tab identity, so it's a single shared handler set.
	onPointerDown: (e: ReactPointerEvent<HTMLDivElement>, roomId: string, harnessId: string) => void;
	onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerUp: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerCancel: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onLostPointerCapture: (e: ReactPointerEvent<HTMLDivElement>) => void;
	suppressClick: () => boolean;
}

export interface HarnessColumnProps {
	room: Room;
	fontSize: number;
	copyOnSelect: boolean;
	defaultShell: string[];
	showPicker: boolean;
	// True iff this column's room is the active room. Combined with
	// `showPicker` and per-harness activeness, it tells each
	// LiveTerminal whether it should hold keyboard focus. Issue #22.
	roomActive: boolean;
	// Drag-and-drop wiring for harness reorder. Pre-resolved against
	// this column's room — `draggedHarnessId` is non-null only when
	// the active drag belongs to *this* room (cross-room drags are
	// rejected upstream). Issue #26.
	harnessDrag: HarnessDrag;
	/** Settings' per-kind default agents (#248), for the picker. */
	defaultAgents: DefaultAgents;
	onPick: (kind: HarnessKind, agent?: string) => void;
	onAddHarness: (roomId: string) => void;
	onCancelPick: () => void;
	onSwitchHarness: (roomId: string, harnessId: string) => void;
	onCloseHarness: (roomId: string, harnessId: string) => void;
	onHarnessCmdChange: (roomId: string, harnessId: string, cmd: string[]) => void;
	// Epic #50 L2c-2: per-opencode-harness embedded-server port. The
	// column pulls each harness's port out of this map (keyed by
	// harnessId) and forwards it to the corresponding LiveTerminal.
	opencodePorts: Map<string, number>;
	// SSE-captured opencode session-id callback (per harness, room
	// scope already bound by App).
	onOpencodeSessionCaptured: (harnessId: string, sessionId: string) => void;
	// #116: opencode followed its TUI onto a different root session
	// (per harness, room scope already bound by App).
	onOpencodeSessionFollowed: (harnessId: string, sessionId: string) => void;
}

export const HarnessColumn = ({
	room,
	fontSize,
	copyOnSelect,
	defaultShell,
	showPicker,
	roomActive,
	harnessDrag,
	defaultAgents,
	onPick,
	onAddHarness,
	onCancelPick,
	onSwitchHarness,
	onCloseHarness,
	onHarnessCmdChange,
	opencodePorts,
	onOpencodeSessionCaptured,
	onOpencodeSessionFollowed,
}: HarnessColumnProps) => {
	const tablistRef = useRef<HTMLDivElement | null>(null);

	// The tab list scrolls; the active tab follows. scrollLeft only —
	// scrollIntoView would also scroll ancestors (design README).
	// offsetLeft is tablist-relative because .sk-harness-tablist is
	// position:relative (the tabs' offsetParent) — without that the
	// values are body-relative and the follow lands ~7px off.
	// Deps include the harness list + roomActive so closing a tab to
	// the left of the active one, or re-showing a hidden room (which
	// can reset the scrollport), re-follows.
	// biome-ignore lint/correctness/useExhaustiveDependencies: room.harnesses is a deliberate trigger — tab removals shift offsets without changing the active id
	useEffect(() => {
		if (!roomActive) return;
		const list = tablistRef.current;
		if (!list) return;
		const el = list.querySelector<HTMLElement>(`[data-htab="${CSS.escape(room.activeHarnessId)}"]`);
		if (!el) return;
		if (el.offsetLeft < list.scrollLeft) {
			list.scrollLeft = el.offsetLeft;
		} else if (el.offsetLeft + el.offsetWidth > list.scrollLeft + list.clientWidth) {
			list.scrollLeft = el.offsetLeft + el.offsetWidth - list.clientWidth;
		}
	}, [room.activeHarnessId, room.harnesses, roomActive]);

	return (
		<div className="sk-harness-col">
			{/* #271: data-drag-strip (+ data-drag-room) covers this whole
			    outer row — tablist, `+ harness`, and the meta text — so a
			    drop anywhere past the last tab resolves to an end-of-strip
			    gap. See the room strip's own data-drag-strip above. */}
			<div className="sk-harness-tabs" data-drag-strip="harness" data-drag-room={room.id}>
				{/* Scrollable list + PINNED add button: without the split, a
				    room with many harnesses pushed `+ harness` off-screen at
				    laptop width (design README — the pinning is load-bearing). */}
				<div className="sk-harness-tablist" ref={tablistRef}>
					{room.harnesses.map((h) => (
						<LiveHarnessTab
							key={h.id}
							h={h}
							active={h.id === room.activeHarnessId}
							closable={room.harnesses.length > 1}
							onClick={() => onSwitchHarness(room.id, h.id)}
							onClose={() => onCloseHarness(room.id, h.id)}
							dragging={harnessDrag.draggedHarnessId === h.id}
							dropSide={harnessDrag.dropTargetHarnessId === h.id ? harnessDrag.dropSide : null}
							dragKind="harness"
							dragId={h.id}
							dragRoomId={room.id}
							onPointerDown={(e) => harnessDrag.onPointerDown(e, room.id, h.id)}
							onPointerMove={harnessDrag.onPointerMove}
							onPointerUp={harnessDrag.onPointerUp}
							onPointerCancel={harnessDrag.onPointerCancel}
							onLostPointerCapture={harnessDrag.onLostPointerCapture}
							suppressClick={harnessDrag.suppressClick}
						/>
					))}
				</div>
				<div className="sk-harness-add" onClick={() => onAddHarness(room.id)}>
					+ harness
				</div>
				<HarnessActionsMenu
					activeHarness={room.harnesses.find((h) => h.id === room.activeHarnessId)}
				/>
				<div className="sk-harness-meta">
					<span>{room.branch ? `${room.repo} · ${room.branch}` : (room.cwd ?? "")}</span>
				</div>
			</div>

			{/*
			 * Mount every harness in this room at once; hide the
			 * inactive ones via display:none so xterm scrollback,
			 * cursor position, and PTY state survive harness-tab
			 * switches inside the room.
			 *
			 * Issue #25: when the picker is up we *also* hide every
			 * harness pane rather than unmounting them — unmounting
			 * fires LiveTerminal's cleanup, which pty_kills the PTY,
			 * which kills the live Claude conversation we're trying
			 * to add a sibling to. The picker takes the flex space
			 * while present; harness panes survive untouched.
			 */}
			{showPicker && (
				<HarnessPicker
					cwd={room.cwd ?? ""}
					defaultAgents={defaultAgents}
					active={roomActive}
					onPick={onPick}
					onCancel={onCancelPick}
				/>
			)}
			{room.harnesses.map((h) => {
				// "Visible" = user can see and interact with this body:
				// room is active, no picker shadowing it, and this is the
				// room's active harness. Drives the focus effect in
				// LiveTerminal (#22).
				const visible = roomActive && !showPicker && h.id === room.activeHarnessId;
				return (
					<div
						key={h.id}
						style={{
							display: visible ? "flex" : "none",
							flexDirection: "column",
							flex: 1,
							minHeight: 0,
							// Pair with `.sk-harness-col`'s overflow:hidden — stops
							// xterm's canvas from pushing this wrapper taller when
							// the terminal font grows (#16).
							overflow: "hidden",
						}}
					>
						{HARNESS_KINDS[h.kind].capabilities.pty ? (
							<HarnessBody
								harness={h}
								fontSize={fontSize}
								copyOnSelect={copyOnSelect}
								defaultShell={defaultShell}
								visible={visible}
								onCmdChange={(newCmd) => onHarnessCmdChange(room.id, h.id, newCmd)}
								roomId={room.id}
								opencodePort={opencodePorts.get(h.id)}
								onSessionCaptured={(sid) => onOpencodeSessionCaptured(h.id, sid)}
								onSessionFollowed={(sid) => onOpencodeSessionFollowed(h.id, sid)}
							/>
						) : (
							<FilesBody harnessId={h.id} cwd={h.cwd ?? room.cwd ?? ""} visible={visible} />
						)}
					</div>
				);
			})}
		</div>
	);
};
