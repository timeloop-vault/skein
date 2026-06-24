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

import {
	isPermissionGranted,
	requestPermission,
	sendNotification,
} from "@choochmeque/tauri-plugin-notifications-api";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { type DragEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CommandPalette, type PaletteItem } from "./CommandPalette.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import { ReopenRoomModal } from "./ReopenRoomModal.tsx";
import { SettingsModal } from "./SettingsModal.tsx";
import { Splitter } from "./Splitter.tsx";
import { HChip, HarnessPicker, HarnessTab, RoomTab, StatusDot } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import {
	activityToStatus,
	effectiveStatus,
	harnessActivity,
	useHarnessActivity,
	useRoomActivity,
} from "./harnessActivity.ts";
import {
	ACTION_EVENT,
	type HarnessAction,
	LiveContext,
	apiErrorToastText,
	parsePayload,
} from "./liveContext/index.ts";
import { usePersistedState } from "./prefs.ts";
import { isAppShortcut, isMac, modLabel } from "./shortcuts.ts";
import type { Density, Harness, HarnessKind, Room, Theme } from "./types.ts";
import { useFocusRestore } from "./useFocusRestore.ts";

// ── Toasts (in-app notifications, L5c) ─────────────────────────────
//
// A toast is the in-app complement to L5b's OS notification: it
// fires when Skein has focus but the user isn't looking at the
// source harness (e.g. they're in a different room when an agent
// finishes). Fixed to the bottom-right corner, click to jump,
// auto-dismiss after a few seconds.

interface ToastEntry {
	id: string;
	roomId: string;
	harnessId: string;
	kind: HarnessKind;
	roomName: string;
	harnessName: string;
	// "waiting" lands here once L2c-1 (Claude JSONL adapter) reports
	// a `last-prompt` row → harness is awaiting user input. Rendered
	// verbatim in the toast subtitle. "error" is the D2f api_error
	// variant — red treatment plus the dim `detail` line.
	state: "idle" | "exited" | "waiting" | "error";
	/** Error variant only: summary under the subtitle, e.g.
	 *  "Overloaded (529), retrying · attempt 4 of 10 · retry in 4.4s". */
	detail?: string | undefined;
}

const TOAST_DISMISS_MS = 6_000;
const TOAST_MAX_VISIBLE = 5;
/// api_error rows within this window count as one incident (a retry
/// burst lands as several rows seconds apart — badge once, not per row).
const API_ERROR_INCIDENT_MS = 60_000;
/// #84: the notification plugin (Swift) crashed (use-after-free in
/// `saveNotification`) after long uptime — its `show` isn't safe to call
/// concurrently, and multiple harness transitions while the user is away
/// fire `sendNotification` from overlapping async tasks. Serialize every
/// OS notification through one promise chain so at most one is ever in
/// flight, removing the concurrency the race needs. Each link swallows
/// its own rejection so one failure (e.g. plugin absent in dev) doesn't
/// stall the chain.
let osNotifyChain: Promise<unknown> = Promise.resolve();
const enqueueOsNotification = (title: string, body: string): void => {
	osNotifyChain = osNotifyChain
		.catch(() => {})
		.then(() => sendNotification({ title, body }))
		.catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn("[skein] sendNotification failed:", msg);
		});
};

/// Per-harness badge coalesce window. A burst of badge-worthy
/// transitions inside this window (a Claude JSONL truncation-replay
/// re-emitting old end_turns — #62; shell prompt-redraw chatter
/// flipping running↔idle — #64) only bumps the count once. Genuine
/// activity spaced further apart than this still increments, and a
/// harness with no pending badge always shows the first one. The
/// underlying replay/dedup at the source is tracked in #93.
const BADGE_COALESCE_MS = 10_000;

const Toast = ({
	toast,
	onClick,
	onDismiss,
}: {
	toast: ToastEntry;
	onClick: () => void;
	onDismiss: () => void;
}) => {
	useEffect(() => {
		const id = setTimeout(onDismiss, TOAST_DISMISS_MS);
		return () => clearTimeout(id);
	}, [onDismiss]);
	return (
		<div
			className={`sk-toast${toast.state === "error" ? " error" : ""}`}
			onClick={onClick}
			title="Go to this harness"
		>
			<HChip kind={toast.kind} size={14} />
			<div className="sk-toast-body">
				<div className="sk-toast-title">{toast.roomName}</div>
				<div className="sk-toast-sub">
					{toast.harnessName} · {toast.state}
				</div>
				{toast.detail && <div className="sk-toast-detail">{toast.detail}</div>}
			</div>
			<span
				className="sk-toast-x"
				title="Dismiss"
				onClick={(e) => {
					e.stopPropagation();
					onDismiss();
				}}
			>
				×
			</span>
		</div>
	);
};

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
	if (!activity) return <HarnessTab {...props} />;
	// Apply the acknowledged-downgrade: a waiting harness with no
	// pending notifications has already been seen, so render it as
	// idle (grey) instead of waiting (blue pulse). The phase in
	// the store stays `waiting` — only the visual indicator
	// collapses.
	const status = effectiveStatus(activity, props.h.pendingNotifications ?? 0);
	return <HarnessTab {...props} h={{ ...props.h, status }} />;
};

// L4 — per-room aggregate. Subscribes to every harness in the
// room; the aggregate priority is waiting > running > idle >
// exited so the dot surfaces the most "alive" state across the
// room's harnesses. `waiting` lands once L2b pattern-matching
// ships.
// L5a — derived badge. The persisted `r.badge` field is vestigial
// now; the visible badge is the sum of each harness's pending
// counter. Overriding it here means every room tab path
// (active, archived list, etc.) shows the right value without
// having to update `r.badge` from notification logic.
const LiveRoomTab = (props: Parameters<typeof RoomTab>[0]) => {
	// Pass the full harness records (not just ids) so the aggregate
	// can apply the same acknowledged-downgrade per harness — a room
	// dot shouldn't pulse for a waiting-but-seen harness.
	const harnessRefs = useMemo(
		() =>
			props.r.harnesses.map((h) => ({ id: h.id, pendingNotifications: h.pendingNotifications })),
		[props.r.harnesses],
	);
	const aggregate = useRoomActivity(harnessRefs);
	const badge = props.r.harnesses.reduce((acc, h) => acc + (h.pendingNotifications ?? 0), 0);
	const derived = { ...props.r, badge, ...(aggregate !== null && { status: aggregate }) };
	return <RoomTab {...props} r={derived} />;
};

const LiveStatusBarChip = ({ harness }: { harness: Harness }) => {
	const activity = useHarnessActivity(harness.id);
	// Dot color uses effectiveStatus so a waiting-but-acknowledged
	// harness renders grey (no pulse) in the bottom bar. The TEXT
	// keeps the underlying phase via activityToStatus — telling the
	// user "idle" when Claude is sitting at a prompt would be a lie;
	// the visual collapse to grey is a UX choice, the text isn't.
	const dotStatus = activity
		? effectiveStatus(activity, harness.pendingNotifications ?? 0)
		: harness.status;
	const label = activity ? activityToStatus(activity) : harness.status;
	return (
		<span className="seg">
			<span className={`dot-tiny st-${dotStatus}`} />
			{label}
		</span>
	);
};

// ── Harness body ───────────────────────────────────────────────────

interface HarnessBodyProps {
	harness: Harness;
	fontSize: number;
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
}

const HarnessBody = ({
	harness,
	fontSize,
	defaultShell,
	visible,
	onCmdChange,
	roomId,
	opencodePort,
	onSessionCaptured,
}: HarnessBodyProps) => {
	if (harness.cmd && harness.cwd !== undefined) {
		// mountKey changes whenever cmd content does — that's the trigger
		// for a clean unmount + remount when the user picks Enter-for-
		// shell after a child exits. Joining the array gives a value-
		// equal string across renders that are content-identical, so a
		// re-render with the same cmd doesn't churn the PTY.
		return (
			<LiveTerminal
				cmd={harness.cmd}
				cwd={harness.cwd}
				mountKey={`${harness.id}:${harness.cmd.join("\x00")}`}
				harnessId={harness.id}
				roomId={roomId}
				harnessKind={harness.kind}
				sessionId={harness.sessionId}
				opencodePort={opencodePort}
				onSessionCaptured={onSessionCaptured}
				fontSize={fontSize}
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

interface HarnessDrag {
	draggedHarnessId: string | null;
	dropTargetHarnessId: string | null;
	dropSide: "before" | "after" | null;
	onDragStart: (e: DragEvent<HTMLDivElement>, roomId: string, harnessId: string) => void;
	onDragOver: (e: DragEvent<HTMLDivElement>, roomId: string, harnessId: string) => void;
	onDrop: (e: DragEvent<HTMLDivElement>, roomId: string, targetId: string) => void;
	onDragEnd: () => void;
}

interface HarnessColumnProps {
	room: Room;
	fontSize: number;
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
	onPick: (kind: HarnessKind) => void;
	onAddHarness: (roomId: string) => void;
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
}

const HarnessColumn = ({
	room,
	fontSize,
	defaultShell,
	showPicker,
	roomActive,
	harnessDrag,
	onPick,
	onAddHarness,
	onSwitchHarness,
	onCloseHarness,
	onHarnessCmdChange,
	opencodePorts,
	onOpencodeSessionCaptured,
}: HarnessColumnProps) => (
	<div className="sk-harness-col">
		<div className="sk-harness-tabs">
			{room.harnesses.map((h) => (
				<LiveHarnessTab
					key={h.id}
					h={h}
					active={h.id === room.activeHarnessId}
					closable={room.harnesses.length > 1}
					onClick={() => onSwitchHarness(room.id, h.id)}
					onClose={() => onCloseHarness(room.id, h.id)}
					draggable
					dragging={harnessDrag.draggedHarnessId === h.id}
					dropSide={harnessDrag.dropTargetHarnessId === h.id ? harnessDrag.dropSide : null}
					onDragStart={(e) => harnessDrag.onDragStart(e, room.id, h.id)}
					onDragOver={(e) => harnessDrag.onDragOver(e, room.id, h.id)}
					onDrop={(e) => harnessDrag.onDrop(e, room.id, h.id)}
					onDragEnd={harnessDrag.onDragEnd}
				/>
			))}
			<div className="sk-harness-add" onClick={() => onAddHarness(room.id)}>
				+ harness
			</div>
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
		{showPicker && <HarnessPicker onPick={onPick} />}
		{room.harnesses.map((h) => {
			// "Visible" = user can see and interact with this terminal:
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
					<HarnessBody
						harness={h}
						fontSize={fontSize}
						defaultShell={defaultShell}
						visible={visible}
						onCmdChange={(newCmd) => onHarnessCmdChange(room.id, h.id, newCmd)}
						roomId={room.id}
						opencodePort={opencodePorts.get(h.id)}
						onSessionCaptured={(sid) => onOpencodeSessionCaptured(h.id, sid)}
					/>
				</div>
			);
		})}
	</div>
);

// xterm font size range. Outside this band the terminal looks either
// unreadable (sub-12) or comically large (above 18) on a 1320x820 window.
const FONT_MIN = 12;
const FONT_MAX = 18;
const FONT_DEFAULT = 13;

// ── New room dialog ────────────────────────────────────────────────
// The picked folder becomes the room's cwd; every harness in the
// room spawns into it. "New worktree" mode resolves to a fresh
// libgit2 worktree path; "Current branch" mode uses the picked path
// as-is.

interface BranchInfoDto {
	name: string;
	isHead: boolean;
}

// What the dialog hands back. The cwd is already the *real* directory
// the spawn should land in — for "New worktree" mode the dialog has
// already called git_add_worktree and resolved the worktree path; for
// "Current branch" mode it's the picked repo path; for non-git rooms
// (chapter 6 phase 3) it's the picked folder verbatim, with branch
// undefined.
interface CreateRoomArgs {
	cwd: string;
	task: string;
	harness: HarnessKind;
	branch?: string;
}

type RepoStatus =
	| { kind: "empty" }
	| { kind: "checking" }
	| { kind: "valid"; branches: BranchInfoDto[]; head: string | null }
	| { kind: "not-a-repo" };

const NewRoomDialog = ({
	defaultCwd,
	onCommit,
	onCancel,
}: {
	defaultCwd: string;
	onCommit: (args: CreateRoomArgs) => void;
	onCancel: () => void;
}) => {
	useFocusRestore();
	const [cwd, setCwd] = useState<string>("");
	const [task, setTask] = useState("");
	const [harness, setHarness] = useState<HarnessKind>("claude");
	const [branchMode, setBranchMode] = useState<"worktree" | "current">("worktree");
	const [baseBranch, setBaseBranch] = useState<string>("");
	const [repoStatus, setRepoStatus] = useState<RepoStatus>({ kind: "empty" });
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	// Validate the picked folder + load branches. Debounced so typing in
	// the path field doesn't fire one round-trip per keystroke.
	useEffect(() => {
		setError(null);
		if (!cwd) {
			setRepoStatus({ kind: "empty" });
			return undefined;
		}
		setRepoStatus({ kind: "checking" });
		const handle = window.setTimeout(() => {
			let cancelled = false;
			(async () => {
				try {
					const isRepo = await invoke<boolean>("git_is_repo", { path: cwd });
					if (cancelled) return;
					if (!isRepo) {
						setRepoStatus({ kind: "not-a-repo" });
						return;
					}
					const branches = await invoke<BranchInfoDto[]>("git_branches", { path: cwd });
					if (cancelled) return;
					const head = branches.find((b) => b.isHead)?.name ?? null;
					setRepoStatus({ kind: "valid", branches, head });
					// Default base branch to HEAD on first valid load.
					setBaseBranch((prev) => prev || head || branches[0]?.name || "");
				} catch (err: unknown) {
					if (cancelled) return;
					const msg = err instanceof Error ? err.message : String(err);
					setError(`git: ${msg}`);
					setRepoStatus({ kind: "not-a-repo" });
				}
			})();
			return () => {
				cancelled = true;
			};
		}, 200);
		return () => window.clearTimeout(handle);
	}, [cwd]);

	const slug =
		task
			.trim()
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, "-")
			.replace(/^-|-$/g, "")
			.slice(0, 28) || "task";
	const proposedBranch = `skein/${slug}`;

	const isRepo = repoStatus.kind === "valid";
	const folderResolved = repoStatus.kind === "valid" || repoStatus.kind === "not-a-repo";
	// Submit is fine for both git-backed and plain folders. The branch /
	// worktree picker only gates submission when the folder *is* a repo.
	const canCreate =
		task.trim().length > 0 &&
		!busy &&
		folderResolved &&
		(!isRepo || branchMode === "current" || baseBranch.length > 0);

	const browse = async () => {
		const start = cwd || defaultCwd;
		const picked = await openDialog({
			directory: true,
			multiple: false,
			title: "Pick a folder for this room",
			...(start ? { defaultPath: start } : {}),
		});
		if (typeof picked === "string") {
			setCwd(picked);
		}
	};

	const submit = async () => {
		if (!canCreate) return;
		setBusy(true);
		setError(null);
		try {
			if (!isRepo) {
				// Non-git folder — no worktree, no branch. cwd is the
				// picked folder verbatim.
				onCommit({
					cwd,
					task: task.trim(),
					harness,
				});
				return;
			}
			if (branchMode === "worktree") {
				const worktreePath = await invoke<string>("git_propose_worktree_path", {
					repoPath: cwd,
					taskSlug: slug,
				});
				const wt = await invoke<{ name: string; path: string }>("git_add_worktree", {
					repoPath: cwd,
					branch: proposedBranch,
					baseBranch,
					worktreePath,
				});
				onCommit({
					cwd: wt.path,
					task: task.trim(),
					harness,
					branch: proposedBranch,
				});
			} else {
				onCommit({
					cwd,
					task: task.trim(),
					harness,
					branch: repoStatus.kind === "valid" ? (repoStatus.head ?? "HEAD") : "HEAD",
				});
			}
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			setError(msg);
			setBusy(false);
		}
	};

	const statusBlurb = (() => {
		switch (repoStatus.kind) {
			case "empty":
				return null;
			case "checking":
				return <span style={{ color: "var(--fg-3)" }}>checking…</span>;
			case "valid":
				return (
					<span style={{ color: "var(--ok)" }}>
						✓ git repo{repoStatus.head ? ` (HEAD: ${repoStatus.head})` : ""}
					</span>
				);
			case "not-a-repo":
				return (
					<span style={{ color: "var(--fg-3)" }}>
						not a git repo — harnesses run in this folder as-is.
					</span>
				);
		}
	})();

	return (
		<div className="sk-modal-bg" onClick={onCancel}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>New room</h2>
					<div className="sub">A room is a folder + task. You can add more harnesses inside.</div>
				</div>
				<div className="sk-modal-body">
					<div className="sk-field">
						<label htmlFor="sk-task">Task</label>
						<input
							// biome-ignore lint/a11y/noAutofocus: modal entrypoint, focus belongs on the task field
							autoFocus
							id="sk-task"
							className="sk-input"
							placeholder="e.g. Wire up the migration runner"
							value={task}
							onChange={(e) => setTask(e.target.value)}
							onKeyDown={(e) => {
								if (e.key === "Enter") void submit();
								if (e.key === "Escape") onCancel();
							}}
						/>
					</div>

					<div className="sk-field">
						<label>Folder</label>
						<div style={{ display: "flex", gap: 6, alignItems: "center" }}>
							<input
								className="sk-input"
								style={{ flex: 1 }}
								placeholder="Pick a folder…"
								value={cwd}
								onChange={(e) => setCwd(e.target.value)}
							/>
							<button className="sk-btn" onClick={browse} type="button">
								Browse…
							</button>
						</div>
						{statusBlurb && (
							<div style={{ fontFamily: "var(--sk-mono)", fontSize: 10.5, marginTop: 2 }}>
								{statusBlurb}
							</div>
						)}
					</div>

					{isRepo && (
						<div className="sk-field">
							<label>Branch</label>
							<div className="sk-radio-row">
								<div
									className={`sk-radio-card ${branchMode === "worktree" ? "selected" : ""}`}
									onClick={() => setBranchMode("worktree")}
								>
									<div className="top">New worktree</div>
									<div className="desc">{proposedBranch}</div>
								</div>
								<div
									className={`sk-radio-card ${branchMode === "current" ? "selected" : ""}`}
									onClick={() => setBranchMode("current")}
								>
									<div className="top">Current branch</div>
									<div className="desc">{repoStatus.head ?? "HEAD"} · in place</div>
								</div>
							</div>
							{branchMode === "worktree" && (
								<div style={{ marginTop: 6 }}>
									<label
										style={{
											fontFamily: "var(--sk-mono)",
											fontSize: 10,
											color: "var(--fg-2)",
											textTransform: "uppercase",
											letterSpacing: "0.08em",
										}}
									>
										Based on
									</label>
									<select
										className="sk-select"
										style={{ marginTop: 4, width: "100%" }}
										value={baseBranch}
										onChange={(e) => setBaseBranch(e.target.value)}
									>
										{repoStatus.branches.map((b) => (
											<option key={b.name} value={b.name}>
												{b.name}
												{b.isHead ? " (HEAD)" : ""}
											</option>
										))}
									</select>
								</div>
							)}
						</div>
					)}

					<div className="sk-field">
						<label>Starting harness</label>
						<div className="sk-radio-row">
							{HARNESS_ORDER.map((id) => {
								const k = HARNESS_KINDS[id];
								return (
									<div
										key={id}
										className={`sk-radio-card ${harness === id ? "selected" : ""}`}
										onClick={() => setHarness(id)}
									>
										<div className="top">
											<HChip kind={id} size={14} /> {k.name}
										</div>
										<div className="desc">{k.desc}</div>
									</div>
								);
							})}
						</div>
					</div>

					{error && (
						<div
							style={{
								color: "var(--err)",
								fontFamily: "var(--sk-mono)",
								fontSize: 11,
								padding: "8px 10px",
								background: "color-mix(in srgb, var(--err) 8%, var(--bg-2))",
								border: "1px solid color-mix(in srgb, var(--err) 35%, var(--line))",
								borderRadius: 5,
							}}
						>
							{error}
						</div>
					)}
				</div>
				<div className="sk-modal-foot">
					<button className="sk-btn" onClick={onCancel}>
						Cancel
					</button>
					<button
						className="sk-btn primary"
						disabled={!canCreate}
						style={{ opacity: canCreate ? 1 : 0.5, cursor: canCreate ? "pointer" : "not-allowed" }}
						onClick={() => void submit()}
					>
						{busy ? "Creating…" : "Create room"}
					</button>
				</div>
			</div>
		</div>
	);
};

// ── Empty state ────────────────────────────────────────────────────

interface EmptyStateProps {
	onNew: () => void;
	archivedCount: number;
	onReopen: () => void;
}

const EmptyState = ({ onNew, archivedCount, onReopen }: EmptyStateProps) => (
	<div className="sk-empty">
		<div className="glyph">⊜</div>
		<h1>No rooms yet</h1>
		<div className="lede">
			A room pins a folder and a task. Open as many harnesses inside as you want — Claude Code and
			opencode on the same worktree, two Copilot runs on a fix, whatever shape the work takes.
		</div>
		<button className="start-btn" onClick={onNew}>
			Create your first room
		</button>
		{archivedCount > 0 && (
			<button type="button" className="sk-empty-reopen" onClick={onReopen}>
				Reopen recent ({archivedCount})…
			</button>
		)}
		<div className="hint-list">
			<div className="row">
				<span className="kbd">{modLabel} N</span>
				<span>New room</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} ⇧ H</span>
				<span>Add harness to current room</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} Tab</span>
				<span>Next room (⇧ for previous, 1-9 for nth)</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} →</span>
				<span>Next harness (⌘ ⇧ → for next room, ⌘ ← / ⌘ ⇧ ← for previous)</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} W</span>
				<span>Close active room</span>
			</div>
		</div>
	</div>
);

// ── Titlebar ───────────────────────────────────────────────────────

// Tauri-driven minimize / toggle-maximize / close buttons. On macOS
// we don't render these — tauri.macos.conf.json sets titleBarStyle:
// "Overlay" and the OS draws real traffic lights at the upper-left.
// On Windows / Linux `decorations: false` means the OS draws nothing,
// so these are the only way to close the window from the UI.
const WindowControls = () => {
	const win = getCurrentWindow();
	return (
		<div className="sk-window-controls" data-tauri-drag-region="false">
			<button
				className="sk-wc-btn"
				onClick={() => void win.minimize()}
				title="Minimize"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M0 5h10" stroke="currentColor" strokeWidth="1" />
				</svg>
			</button>
			<button
				className="sk-wc-btn"
				onClick={() => void win.toggleMaximize()}
				title="Maximize"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<rect
						x="0.5"
						y="0.5"
						width="9"
						height="9"
						fill="none"
						stroke="currentColor"
						strokeWidth="1"
					/>
				</svg>
			</button>
			<button
				className="sk-wc-btn sk-wc-close"
				onClick={() => void win.close()}
				title="Close"
				type="button"
			>
				<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
					<path d="M1 1l8 8M9 1l-8 8" stroke="currentColor" strokeWidth="1" />
				</svg>
			</button>
		</div>
	);
};

interface TitlebarProps {
	activeRoomLabel: string | null;
	onOpenSettings: () => void;
}

const Titlebar = ({ activeRoomLabel, onOpenSettings }: TitlebarProps) => (
	<div className="sk-titlebar" data-tauri-drag-region>
		<span className="sk-app-name">
			<span className="dot">●</span> skein
		</span>
		{activeRoomLabel && <span className="sk-titlebar-session">{activeRoomLabel}</span>}
		<div className="sk-titlebar-actions" data-tauri-drag-region="false">
			<button
				type="button"
				className="sk-cog-btn"
				onClick={onOpenSettings}
				title={`Settings (${modLabel}+,)`}
				aria-label="Settings"
			>
				<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
					<path
						d="M8 5.5a2.5 2.5 0 100 5 2.5 2.5 0 000-5zm5.6 2.5l1.4-.9-1.4-2.4-1.6.5a5.5 5.5 0 00-1.5-.9l-.3-1.7h-2.8l-.3 1.7c-.55.22-1.05.52-1.5.9l-1.6-.5-1.4 2.4 1.4.9c-.07.3-.1.6-.1.9s.03.6.1.9l-1.4.9 1.4 2.4 1.6-.5c.45.38.95.68 1.5.9l.3 1.7h2.8l.3-1.7c.55-.22 1.05-.52 1.5-.9l1.6.5 1.4-2.4-1.4-.9c.07-.3.1-.6.1-.9s-.03-.6-.1-.9z"
						fill="none"
						stroke="currentColor"
						strokeWidth="1.1"
						strokeLinejoin="round"
					/>
				</svg>
			</button>
		</div>
		{!isMac && <WindowControls />}
	</div>
);

// ── App ────────────────────────────────────────────────────────────

const newId = (prefix: string): string => prefix + Math.random().toString(36).slice(2, 7);

// Mapping from harness kind → argv. Each binary must be on PATH for the
// spawn to succeed; if it isn't, the LiveTerminal renders the error
// inline and the user can pick another kind. The `byoh` kind is our
// "Shell" option — it drops into the user's default shell so they can
// run whatever they want.
// Phase 2b: opencode has no Claude-style --session-id pre-allocation.
// Snapshot opencode's existing sessions for this cwd, spawn the
// harness, then poll the same query looking for an id that wasn't in
// the snapshot AND isn't already claimed by some other Skein harness.
// First match wins; that's this harness's session.
//
// Why polling at all (not a file/db watcher): the capture window is
// short relative to a session lifetime, and opencode writes the row
// once. Watcher's lifetime cost > polling's burst.
//
// Why a long timeout (5 minutes): opencode appears to write the
// session row only on the first user input, not at spawn — so a
// short window misses it whenever the user takes a beat to start
// typing. 5 min covers nearly every realistic case; on timeout we
// quietly leave sessionId undefined and resume falls back to
// phase-5a's --continue.
//
// `claimedIds` returns the set of session ids any *other* harness
// has already captured. If two opencode harnesses spawn in the same
// cwd within seconds, the snapshot diff alone can't tell them apart;
// excluding already-claimed ids breaks the tie deterministically.
const captureOpencodeSessionId = async (
	cwd: string,
	claimedIds: () => Set<string>,
	onCapture: (sessionId: string) => void,
): Promise<void> => {
	let snapshot: string[];
	try {
		snapshot = await invoke<string[]>("opencode_list_sessions", { cwd });
	} catch (err) {
		console.warn("[skein] opencode capture: snapshot failed", err);
		return;
	}
	const before = new Set(snapshot);
	const startedAt = Date.now();
	const deadline = startedAt + 5 * 60 * 1000;
	console.info(`[skein] opencode capture started for ${cwd} (snapshot ${before.size} sessions)`);
	while (Date.now() < deadline) {
		// Backoff: tight (250 ms) for the first 5 s in case opencode
		// is fast, then 1 s for the next 25 s, then 5 s thereafter.
		const elapsed = Date.now() - startedAt;
		const waitMs = elapsed < 5_000 ? 250 : elapsed < 30_000 ? 1_000 : 5_000;
		await new Promise((resolve) => setTimeout(resolve, waitMs));
		try {
			const current = await invoke<string[]>("opencode_list_sessions", { cwd });
			const taken = claimedIds();
			const fresh = current.find((id) => !before.has(id) && !taken.has(id));
			if (fresh) {
				console.info(`[skein] opencode capture: ${fresh} (${cwd})`);
				onCapture(fresh);
				return;
			}
		} catch {
			// Transient — try again on the next tick.
		}
	}
	console.warn(`[skein] opencode capture timed out for ${cwd}`);
};

// Phase 2a: when sessionId is provided (always set by callers for
// Claude, never for other kinds), pre-allocate Claude's conversation
// id via --session-id <uuid>. Storing the same id on the harness
// record lets phase 3 resume directly with no picker.
//
// Epic #50 L2c-2: opencode embeds an HTTP server. Skein allocates a
// free port up front and passes `--port <N> --hostname 127.0.0.1` so
// the L2c-2 SSE adapter knows where to subscribe. The port is fresh
// per spawn (not persisted) — callers must pass one in for opencode.
const cmdForKind = (
	kind: HarnessKind,
	fallbackShell: string[],
	sessionId?: string,
	opencodePort?: number,
): string[] => {
	switch (kind) {
		case "claude":
			return sessionId ? ["claude", "--session-id", sessionId] : ["claude"];
		case "opencode": {
			// Default port=0 lets opencode pick; that defeats the whole
			// adapter, so require an allocated port. If a caller forgot,
			// fall back to bare opencode and the adapter just won't
			// attach — same behaviour as pre-L2c-2.
			if (opencodePort === undefined) return ["opencode"];
			return ["opencode", "--port", String(opencodePort), "--hostname", "127.0.0.1"];
		}
		case "copilot":
			return ["gh", "copilot", "suggest"];
		case "byoh":
			return fallbackShell.length > 0 ? fallbackShell : ["pwsh.exe"];
	}
};

// Rewrite a stored harness into its "resume the previous conversation"
// form, applied once at boot so a fresh PTY spawn transparently
// re-attaches. Only matches harnesses whose cmd still looks like a
// freshly-spawned Claude / opencode launch — anything customized
// (shell-swapped via chapter 2's onCmdChange, user-edited extra args,
// or already in resume form from a previous Skein boot) passes through
// unchanged.
//
// Three sources of session ids feed in:
//   1. harness.sessionId set by phase 2a (Claude pre-allocate).
//   2. harness.sessionId set by phase 2b (opencode capture-after-spawn).
//   3. None — legacy harness created before chapter 5, or capture
//      timed out. We fall back to chapter 2 phase 5a's behaviour:
//      Claude shows its picker, opencode resumes most-recent-in-cwd.
//
// gh copilot has no resume mode; shells start fresh; both pass through.
//
// Epic #50 L2c-2: opencode resume always injects a fresh port. The
// old port from sqlite is dead — the previous Skein run released it
// when the harness exited. We "fresh-form" check is liberal: any
// opencode cmd that hasn't been shell-swapped (cmd[0] === "opencode")
// gets the resume rewrite. That covers (a) legacy `["opencode"]`
// records from pre-L2c-2, (b) cmds with --port from a previous boot,
// and (c) cmds already in --session/--continue form.
const resumeCmd = (h: Harness, opencodePort?: number): string[] => {
	const cmd = h.cmd ?? [];
	if (h.kind === "claude") {
		const isFreshClaude =
			(cmd.length === 1 && cmd[0] === "claude") ||
			(cmd.length === 3 && cmd[0] === "claude" && cmd[1] === "--session-id");
		if (isFreshClaude) {
			return h.sessionId ? ["claude", "--resume", h.sessionId] : ["claude", "--resume"];
		}
	}
	if (h.kind === "opencode" && cmd[0] === "opencode") {
		const args = ["opencode"];
		if (opencodePort !== undefined) {
			args.push("--port", String(opencodePort), "--hostname", "127.0.0.1");
		}
		if (h.sessionId) args.push("--session", h.sessionId);
		else args.push("--continue");
		return args;
	}
	return cmd;
};

export default function App() {
	const [theme, setTheme] = usePersistedState<Theme>("theme", "dark");
	const [density, setDensity] = usePersistedState<Density>("density", "regular");
	const [fontSize, setFontSize] = usePersistedState<number>("fontSize", FONT_DEFAULT);
	// L5e — per-surface notification toggles. Defaults: in-app on,
	// OS off (less surprising on first run; user opts in to OS
	// banners when they want them).
	const [notifyBadge, setNotifyBadge] = usePersistedState<boolean>("notifyBadge", true);
	const [notifyToast, setNotifyToast] = usePersistedState<boolean>("notifyToast", true);
	const [notifyUrgent, setNotifyUrgent] = usePersistedState<boolean>("notifyUrgent", true);
	const [notifyOs, setNotifyOs] = usePersistedState<boolean>("notifyOs", false);
	// Per-turn cost hair-lines in the Activity feed (issue #80 D2d-2).
	// Off by default; toggled from the Activity card head. App-owned so
	// every room's mounted LiveContext sees the same value.
	const [showTurnCosts, setShowTurnCosts] = usePersistedState<boolean>("showTurnCosts", false);
	// Width of the harness column in px. Right pane absorbs the remainder
	// via flex:1. Splitter clamps against window size at drag time.
	const [harnessColWidth, setHarnessColWidth] = usePersistedState<number>("harnessColWidth", 640);

	const [rooms, setRooms] = useState<Room[]>([]);
	const [activeRoomId, setActiveRoomId] = useState<string>("");
	// Epic #50 L2c-2: per-opencode-harness embedded-server port.
	// Ephemeral — each spawn gets a fresh port via `pick_free_port`,
	// kept here so LiveTerminal can pass it to attachOpencodeEvents
	// without re-allocating. Not persisted: a port is meaningless
	// after the process that bound it dies.
	const [opencodePorts, setOpencodePorts] = useState<Map<string, number>>(new Map());
	// L5c — in-app toasts. Ephemeral (no DB mirror) since they
	// represent "right now, look here" state that doesn't survive
	// a restart. Capped at TOAST_MAX_VISIBLE so a burst of
	// transitions doesn't cover the screen.
	const [toasts, setToasts] = useState<ToastEntry[]>([]);
	// Live HEAD branch per room, populated by LiveStatus on every watcher
	// tick. `room.branch` is the *creation* branch (worktree identity);
	// this is what's actually checked out right now. The status bar reads
	// from here first so a `git checkout` inside a harness is visible.
	// Issue #18.
	const [liveBranches, setLiveBranches] = useState<Record<string, string | null>>({});
	// Stable callbacks for the per-room LiveContext (memoized below): a
	// room switch re-renders App, and without stable props React.memo
	// can't skip the rooms whose `visible` didn't change — every mounted
	// room would reconcile its whole feed. setShowTurnCosts is stable;
	// onBranchChange takes the roomId so one callback serves all rooms.
	const handleToggleTurnCosts = useCallback(() => setShowTurnCosts((v) => !v), [setShowTurnCosts]);
	const handleBranchChange = useCallback((roomId: string, branch: string | null) => {
		setLiveBranches((prev) => (prev[roomId] === branch ? prev : { ...prev, [roomId]: branch }));
	}, []);
	// Drag-and-drop reorder state. `drag` tracks what's being dragged
	// (a whole room, or a harness scoped to its room — cross-room
	// harness drops are rejected). `dropTarget` tracks where the drop
	// indicator should render, updated by every dragOver. Issue #26.
	//
	// `dragRef` mirrors `drag` synchronously so the dragOver handler
	// can read it without waiting for React to commit the state update
	// from dragstart. Two-channel design: React state drives the UI
	// (`dragging` opacity, drop indicator), the ref drives the drag-
	// kind check in dragOver / drop. Using DataTransfer.types for this
	// doesn't work in WebKit (Tauri's macOS engine) because custom MIME
	// types aren't exposed until the drop event fires for privacy
	// reasons — checking in dragOver always returns false, so
	// preventDefault is skipped and the drop is silently rejected.
	type DragInfo = { kind: "room"; id: string } | { kind: "harness"; roomId: string; id: string };
	const [drag, setDrag] = useState<DragInfo | null>(null);
	const dragRef = useRef<DragInfo | null>(null);
	const [dropTarget, setDropTarget] = useState<
		| { kind: "room"; id: string; side: "before" | "after" }
		| { kind: "harness"; roomId: string; id: string; side: "before" | "after" }
		| null
	>(null);
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [showNewRoom, setShowNewRoom] = useState(false);
	const [showPalette, setShowPalette] = useState(false);
	const [showSettings, setShowSettings] = useState(false);
	const [showReopen, setShowReopen] = useState(false);

	// Chapter 6 phase 2: split rooms into active (rendered as tabs) and
	// archived (hidden, listed in the reopen modal). Tab strip, command
	// palette, the room useMemo below, and Mod+1..9 all key off active.
	const activeRooms = useMemo(() => rooms.filter((r) => !r.archived), [rooms]);
	const archivedRooms = useMemo(
		() =>
			rooms
				.filter((r) => r.archived)
				.slice()
				.sort((a, b) => (b.archived ?? 0) - (a.archived ?? 0)),
		[rooms],
	);

	// Phase 1: pull platform defaults once at boot. New harnesses spawn
	// into these until Phase 4 wires real worktrees / per-room cwd.
	const [defaultShell, setDefaultShell] = useState<string[]>([]);
	const [defaultCwd, setDefaultCwd] = useState<string>("");
	useEffect(() => {
		void invoke<string[]>("default_shell").then(setDefaultShell);
		void invoke<string>("default_cwd").then(setDefaultCwd);
	}, []);

	// Phase 3: hydrate rooms from sqlite on boot. Until that round-trips,
	// `loaded` stays false and the auto-save effect below stays parked —
	// otherwise the empty initial state would clobber the DB before we read it.
	const [loaded, setLoaded] = useState(false);
	useEffect(() => {
		// Chapter 5 phase 4: drop any stored sessionId that no longer
		// exists on disk before resumeCmd uses it. claude --resume <id>
		// or opencode --session <id> against a deleted conversation
		// would either error or attach to nothing useful; falling back
		// to picker / --continue is the safer default.
		//
		// Errors from the existence checks are conservative: keep the
		// id (return true). The follow-up resume might fail noisily,
		// but at least we don't drop a legitimate id over a transient
		// rusqlite or fs hiccup.
		const stillExists = async (h: Harness): Promise<boolean> => {
			if (!h.sessionId) return true;
			try {
				if (h.kind === "claude") {
					return await invoke<boolean>("claude_session_exists", { id: h.sessionId });
				}
				if (h.kind === "opencode") {
					return await invoke<boolean>("opencode_session_exists", { id: h.sessionId });
				}
			} catch (err) {
				console.warn(`[skein] ${h.kind} session_exists failed for ${h.sessionId}:`, err);
				return true;
			}
			return true;
		};

		invoke<Room[]>("db_load_rooms")
			.then(async (rows) => {
				if (rows.length > 0) {
					const verified = await Promise.all(
						rows.map(async (s) => ({
							...s,
							harnesses: await Promise.all(
								s.harnesses.map(async (h) => {
									if (await stillExists(h)) return h;
									console.info(
										`[skein] dropping stale ${h.kind} sessionId ${h.sessionId} on harness ${h.id}`,
									);
									const { sessionId, ...rest } = h;
									return rest;
								}),
							),
						})),
					);
					// Epic #50 L2c-2: pre-allocate fresh embedded-server
					// ports for every resumed opencode harness. The
					// previous Skein run released its ports on exit;
					// resumeCmd needs the new ones to bake into the
					// argv. Awaiting in parallel keeps boot fast even
					// with many opencode rooms.
					const opencodeHarnesses = verified.flatMap((r) =>
						r.harnesses.filter((h) => h.kind === "opencode" && h.cmd),
					);
					const allocations = await Promise.all(
						opencodeHarnesses.map(async (h) => {
							try {
								const port = await invoke<number>("pick_free_port");
								return [h.id, port] as const;
							} catch (err) {
								console.warn(
									`[skein] pick_free_port failed for ${h.id}; L2c-2 adapter disabled for this harness`,
									err,
								);
								return null;
							}
						}),
					);
					const portMap = new Map<string, number>(
						allocations.filter((a): a is readonly [string, number] => a !== null),
					);
					setOpencodePorts(portMap);
					// Rewrite each harness's cmd to its resume form before
					// mounting, so the PTY spawn re-attaches to the prior
					// conversation instead of starting fresh.
					const withResume = verified.map((r) => ({
						...r,
						harnesses: r.harnesses.map((h) =>
							h.cmd ? { ...h, cmd: resumeCmd(h, portMap.get(h.id)) } : h,
						),
					}));
					setRooms(withResume);
					// Pick the first *active* room; archived ones aren't
					// supposed to be the boot-time selection.
					const first = withResume.find((r) => !r.archived);
					if (first) setActiveRoomId(first.id);
				}
				setLoaded(true);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] db_load_rooms failed:", msg);
				setLoaded(true);
			});
	}, []);

	// Phase 3: any time `rooms` changes after the initial load, mirror
	// the new state to sqlite. Wipe-and-insert is fine at prototype scale.
	useEffect(() => {
		if (!loaded) return;
		void invoke("db_save_rooms", { rooms }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.error("[skein] db_save_rooms failed:", msg);
		});
	}, [rooms, loaded]);

	const room = useMemo(() => rooms.find((r) => r.id === activeRoomId), [rooms, activeRoomId]);
	const activeHarness = room?.harnesses.find((h) => h.id === room.activeHarnessId);

	const switchRoom = (id: string) => {
		setActiveRoomId(id);
		// Pending-notification clearing for the now-displayed harness
		// is handled by a useEffect below — it covers every path
		// that changes the (active room, active harness) tuple, not
		// just tab clicks (Mod+1..9, Mod+Tab, palette, initial load).
	};

	const closeRoom = async (id: string) => {
		// Confirm before close — rooms can hold a lot of state and the
		// prototype has no undo (well, now there's the reopen modal —
		// but the user shouldn't have to discover that). Tauri's
		// plugin-dialog gives us a native confirm; window.confirm is
		// silently no-op'd in WebKit without a host-side handler.
		const ok = await confirm("Close this room? Any running harnesses will be killed.", {
			title: "Skein",
			kind: "warning",
		});
		if (!ok) return;
		// Chapter 6 phase 2: archive instead of delete. Tab strip filters
		// archived out; reopen modal lists them.
		setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, archived: Date.now() } : r)));
		if (id === activeRoomId) {
			const nextActive = activeRooms.find((r) => r.id !== id);
			setActiveRoomId(nextActive ? nextActive.id : "");
		}
	};

	const reopenRoom = (id: string) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== id) return r;
				const { archived, ...rest } = r;
				return rest;
			}),
		);
		setActiveRoomId(id);
		setShowReopen(false);
	};

	// #89: permanently drop an archived room. `db_save_rooms` is a full
	// DELETE + re-insert of the current `rooms` array, so removing it
	// from state *is* the delete — the autosave effect mirrors it out.
	// (Orphaned `harness_actions`/`harness_events` rows for the room are
	// harmless activity-log leftovers; a later pass can vacuum them.)
	const deleteRoomForever = (id: string) => {
		setRooms((prev) => prev.filter((r) => r.id !== id));
	};

	// #89: undo a just-deleted room — re-insert it with its `archived`
	// flag intact, so it returns to the reopen list (not the tab strip).
	// The archivedRooms memo re-sorts it back into place.
	const restoreRoom = (room: Room) => {
		setRooms((prev) => (prev.some((r) => r.id === room.id) ? prev : [...prev, room]));
	};

	const switchHarnessInRoom = (roomId: string, harnessId: string) => {
		setRooms((prev) =>
			prev.map((r) => (r.id === roomId ? { ...r, activeHarnessId: harnessId } : r)),
		);
	};

	// Jump to a specific harness: activate its room AND focus it within
	// that room. Every click-to-jump surface (toast, status-bar urgent
	// indicator, future inbox) should land on the harness that wanted
	// attention, not just its room (#65).
	const jumpToHarness = (roomId: string, harnessId: string) => {
		setActiveRoomId(roomId);
		switchHarnessInRoom(roomId, harnessId);
	};

	const dismissToast = (id: string) => {
		setToasts((prev) => prev.filter((t) => t.id !== id));
	};

	const jumpToToast = (toast: ToastEntry) => {
		jumpToHarness(toast.roomId, toast.harnessId);
		dismissToast(toast.id);
	};

	const closeHarness = (roomId: string, harnessId: string) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== roomId) return r;
				const remaining = r.harnesses.filter((h) => h.id !== harnessId);
				if (remaining.length === 0) return r;
				const first = remaining[0];
				if (!first) return r;
				return { ...r, harnesses: remaining, activeHarnessId: first.id };
			}),
		);
	};

	// When a harness's child exits and the user picks the shell-fallback
	// path, LiveTerminal calls this so the new cmd persists to the DB
	// and a Skein restart re-spawns the shell.
	const updateHarnessCmd = (roomId: string, harnessId: string, cmd: string[]) => {
		setRooms((prev) =>
			prev.map((r) =>
				r.id === roomId
					? { ...r, harnesses: r.harnesses.map((h) => (h.id === harnessId ? { ...h, cmd } : h)) }
					: r,
			),
		);
	};

	const addHarness = (roomId: string) => setShowPicker(roomId);

	// Issue #26: drag-and-drop reorder helpers. ID-based to keep the
	// active-room and active-harness pointers correct after the move —
	// they're already ID-keyed, so the array shuffle doesn't need any
	// extra bookkeeping. Splicing in two steps (remove, then insert)
	// requires the index adjustment when `from < target`: removing
	// the source shifts every later index by one.
	const reorderRoom = (fromId: string, targetId: string, side: "before" | "after") => {
		setRooms((prev) => {
			const fromIdx = prev.findIndex((r) => r.id === fromId);
			const targetIdx = prev.findIndex((r) => r.id === targetId);
			if (fromIdx < 0 || targetIdx < 0 || fromId === targetId) return prev;
			const adjustedTarget = side === "after" ? targetIdx + 1 : targetIdx;
			const insertIdx = fromIdx < adjustedTarget ? adjustedTarget - 1 : adjustedTarget;
			if (fromIdx === insertIdx) return prev;
			const next = [...prev];
			const [item] = next.splice(fromIdx, 1);
			if (!item) return prev;
			next.splice(insertIdx, 0, item);
			return next;
		});
	};

	const reorderHarness = (
		roomId: string,
		fromId: string,
		targetId: string,
		side: "before" | "after",
	) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== roomId) return r;
				const fromIdx = r.harnesses.findIndex((h) => h.id === fromId);
				const targetIdx = r.harnesses.findIndex((h) => h.id === targetId);
				if (fromIdx < 0 || targetIdx < 0 || fromId === targetId) return r;
				const adjustedTarget = side === "after" ? targetIdx + 1 : targetIdx;
				const insertIdx = fromIdx < adjustedTarget ? adjustedTarget - 1 : adjustedTarget;
				if (fromIdx === insertIdx) return r;
				const harnesses = [...r.harnesses];
				const [item] = harnesses.splice(fromIdx, 1);
				if (!item) return r;
				harnesses.splice(insertIdx, 0, item);
				return { ...r, harnesses };
			}),
		);
	};

	const dropSideForRoom = (e: DragEvent<HTMLDivElement>): "before" | "after" => {
		const rect = e.currentTarget.getBoundingClientRect();
		return e.clientX < rect.left + rect.width / 2 ? "before" : "after";
	};

	const handleRoomDragStart = (e: DragEvent<HTMLDivElement>, roomId: string) => {
		const info: DragInfo = { kind: "room", id: roomId };
		dragRef.current = info;
		setDrag(info);
		e.dataTransfer.effectAllowed = "move";
		// Some browsers require setData for a drag to "register"
		// properly; the value is unused since dragRef carries the
		// real intent.
		e.dataTransfer.setData("text/plain", roomId);
	};

	const handleRoomDragOver = (e: DragEvent<HTMLDivElement>, roomId: string) => {
		// Gate preventDefault on the drag kind so cross-kind drags
		// (e.g. dragging a file from outside the app over a tab) get
		// the no-drop cursor automatically. dragRef is set
		// synchronously in dragstart so this check is reliable —
		// React state lag isn't an issue here.
		if (dragRef.current?.kind !== "room") return;
		e.preventDefault();
		e.dataTransfer.dropEffect = "move";
		const side = dropSideForRoom(e);
		setDropTarget((prev) =>
			prev?.kind === "room" && prev.id === roomId && prev.side === side
				? prev
				: { kind: "room", id: roomId, side },
		);
	};

	const handleRoomDrop = (e: DragEvent<HTMLDivElement>, targetId: string) => {
		e.preventDefault();
		const info = dragRef.current;
		if (info?.kind !== "room") return;
		const side = dropSideForRoom(e);
		reorderRoom(info.id, targetId, side);
	};

	const handleHarnessDragStart = (
		e: DragEvent<HTMLDivElement>,
		roomId: string,
		harnessId: string,
	) => {
		const info: DragInfo = { kind: "harness", roomId, id: harnessId };
		dragRef.current = info;
		setDrag(info);
		e.dataTransfer.effectAllowed = "move";
		e.dataTransfer.setData("text/plain", harnessId);
	};

	const handleHarnessDragOver = (
		e: DragEvent<HTMLDivElement>,
		roomId: string,
		harnessId: string,
	) => {
		// Cross-room harness drags get the no-drop cursor for free:
		// roomId mismatch → no preventDefault → browser refuses the
		// drop. Same gating principle as handleRoomDragOver.
		const info = dragRef.current;
		if (info?.kind !== "harness" || info.roomId !== roomId) return;
		e.preventDefault();
		e.dataTransfer.dropEffect = "move";
		const side = dropSideForRoom(e);
		setDropTarget((prev) =>
			prev?.kind === "harness" &&
			prev.roomId === roomId &&
			prev.id === harnessId &&
			prev.side === side
				? prev
				: { kind: "harness", roomId, id: harnessId, side },
		);
	};

	const handleHarnessDrop = (e: DragEvent<HTMLDivElement>, roomId: string, targetId: string) => {
		e.preventDefault();
		const info = dragRef.current;
		if (info?.kind !== "harness" || info.roomId !== roomId) return;
		const side = dropSideForRoom(e);
		reorderHarness(roomId, info.id, targetId, side);
	};

	const handleDragEnd = () => {
		dragRef.current = null;
		setDrag(null);
		setDropTarget(null);
	};

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
	const roomsRef = useRef(rooms);
	roomsRef.current = rooms;
	// Keyboard nav (Mod+Tab, Mod+1..9) keys off active rooms only —
	// archived ones aren't rendered as tabs and shouldn't be reachable
	// via the cycle / jump shortcuts.
	const activeRoomsRef = useRef(activeRooms);
	activeRoomsRef.current = activeRooms;
	const activeRoomIdRef = useRef(activeRoomId);
	activeRoomIdRef.current = activeRoomId;
	// L5e — notification toggles read inside the transition listener
	// (mounted once with empty deps); refs let preference toggles
	// take effect without re-subscribing.
	const notifyBadgeRef = useRef(notifyBadge);
	notifyBadgeRef.current = notifyBadge;
	const notifyToastRef = useRef(notifyToast);
	notifyToastRef.current = notifyToast;
	const notifyOsRef = useRef(notifyOs);
	notifyOsRef.current = notifyOs;
	// D2f — last api_error arrival per harness, for coalescing a retry
	// burst into one badge-worthy incident.
	const lastApiErrorAtRef = useRef<Map<string, number>>(new Map());
	// Per-harness time of the last badge bump, for the coalesce window
	// (#62/#64 — collapse a burst/chatter of transitions into one badge).
	const lastBadgeAtRef = useRef<Map<string, number>>(new Map());

	// L5b — window-focus state + OS-notification permission. The
	// notification logic below skips firing an OS banner when Skein
	// is the focused app, because the user is already looking at
	// the badge update in real time and an extra OS-level banner is
	// just noise. Refs (not state) since the transition callback
	// reads these synchronously and we don't want them to retrigger
	// the subscription effect on every focus change.
	const windowFocusedRef = useRef(true);
	const notificationPermissionRef = useRef<"unknown" | "granted" | "denied">("unknown");
	useEffect(() => {
		const win = getCurrentWindow();
		let unlisten: (() => void) | null = null;
		// Clear the badge on the currently-displayed harness. Used
		// both by the (activeRoomId, displayedHarnessId) effect below
		// AND by the focus listener — coming back to Skein on the
		// same harness you alt+tabbed away from also counts as
		// "viewing it now," but that effect doesn't re-fire because
		// neither tuple value changed. Hook the focus→true edge
		// instead.
		const clearDisplayedHarnessPending = () => {
			const room = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
			if (!room) return;
			const displayedH = room.activeHarnessId;
			if (!displayedH) return;
			setRooms((prev) =>
				prev.map((r) => {
					if (r.id !== room.id) return r;
					const target = r.harnesses.find((h) => h.id === displayedH);
					if (!target || (target.pendingNotifications ?? 0) === 0) return r;
					return {
						...r,
						harnesses: r.harnesses.map((h) =>
							h.id === displayedH ? { ...h, pendingNotifications: 0 } : h,
						),
					};
				}),
			);
		};
		void win.isFocused().then((f) => {
			windowFocusedRef.current = f;
		});
		void win
			.onFocusChanged(({ payload }) => {
				windowFocusedRef.current = payload;
				if (payload) clearDisplayedHarnessPending();
			})
			.then((u) => {
				unlisten = u;
			});
		// Permission flow: prompt once on first run if the user
		// hasn't decided yet. The OS remembers the choice and
		// future `isPermissionGranted` calls return granted/denied
		// without re-prompting.
		void (async () => {
			try {
				const granted = await isPermissionGranted();
				if (granted) {
					notificationPermissionRef.current = "granted";
					return;
				}
				const result = await requestPermission();
				notificationPermissionRef.current = result === "granted" ? "granted" : "denied";
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] notification permission flow failed:", msg);
			}
		})();
		return () => {
			unlisten?.();
		};
	}, []);

	// L5a — pending-notification accounting. A harness transitioning
	// from working (spawning|running) to passive (idle|exited) bumps
	// its own `pendingNotifications` counter — unless it's the
	// harness the user is currently viewing (active room's active
	// harness), in which case we skip because the user can already
	// see the dot change. Same harness in the active room but in a
	// non-active harness tab WILL bump — its tab isn't visible.
	// Room.badge is rendered as the sum across harnesses by
	// LiveRoomTab; we don't write to it here. Counters persist via
	// the rooms→sqlite mirror so the badge survives a restart.
	//
	// L5b — OS notification. Same predicates as the badge bump
	// (passive transition + not the viewed harness + hasUserInput),
	// PLUS Skein is not the focused app. If Skein is focused the
	// badge update is already visible and an OS banner would just
	// duplicate it. Permission is granted lazily on first launch.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to) => {
			// Two trigger classes:
			// • `running|idle → waiting` — a harness-native adapter
			//   (L2c) reported the agent went from doing work to
			//   awaiting user input. Notify-worthy regardless of
			//   hasUserInput, since "Claude is now blocked on you"
			//   is real news even for a freshly-spawned harness
			//   (e.g. first-launch trust prompts).
			//   `spawning → waiting` is excluded: that's the
			//   synthetic initial-state transition the adapter
			//   emits when probing the JSONL on attach. Pre-existing
			//   waiting state isn't a notification — it was true
			//   before Skein started and the blue dot itself
			//   conveys it. Without this gate every Skein restart
			//   would badge every Claude room that was sitting at
			//   a prompt before shutdown.
			// • working → passive (running|spawning → idle|exited).
			//   The pre-L2c surface: agent went quiet. We keep the
			//   hasUserInput gate so the spawn-banner cycle on
			//   every Skein restart doesn't light up every room.
			const becameWaiting = to === "waiting" && (from === "running" || from === "idle");
			const wasWorking = from === "running" || from === "spawning";
			const becamePassive = to === "idle" || to === "exited";
			if (!becameWaiting && !(wasWorking && becamePassive)) return;
			const a = harnessActivity.get(harnessId);
			if (!a) return;
			// hasUserInput gate applies only to the passive transition.
			// `→ waiting` from L2c is unconditional.
			if (!becameWaiting && !a.hasUserInput) return;
			const activeRoom = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
			const isViewedHarness = Boolean(activeRoom && activeRoom.activeHarnessId === harnessId);
			const isWindowFocused = windowFocusedRef.current;
			const owningRoom = roomsRef.current.find((r) => r.harnesses.some((h) => h.id === harnessId));
			// Badge: skip when the user is staring at this exact
			// harness — the tab dot color change tells them what
			// happened. If they alt+tabbed away, though, bump
			// anyway so they see "something happened while I was
			// gone" when they come back. Also skip when the badge
			// surface is disabled in Settings (L5e).
			//
			// Coalesce window (#62/#64): a burst of badge-worthy
			// transitions within BADGE_COALESCE_MS only bumps once, so a
			// Claude JSONL replay or shell redraw-chatter can't pile up
			// 31 phantom badges. The first badge on a clean harness
			// (pending 0) always shows; genuine activity spaced past the
			// window still increments. Record the time on every
			// badge-worthy transition (skipped or not) so continuous
			// sub-window chatter never crosses the window.
			const nowMs = Date.now();
			const curPending =
				owningRoom?.harnesses.find((h) => h.id === harnessId)?.pendingNotifications ?? 0;
			const lastBadgeAt = lastBadgeAtRef.current.get(harnessId) ?? 0;
			const coalesced = curPending > 0 && nowMs - lastBadgeAt < BADGE_COALESCE_MS;
			lastBadgeAtRef.current.set(harnessId, nowMs);
			if (notifyBadgeRef.current && !(isViewedHarness && isWindowFocused) && !coalesced) {
				setRooms((prev) =>
					prev.map((r) => {
						if (!r.harnesses.some((h) => h.id === harnessId)) return r;
						return {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === harnessId
									? { ...h, pendingNotifications: (h.pendingNotifications ?? 0) + 1 }
									: h,
							),
						};
					}),
				);
			}
			const harness = owningRoom?.harnesses.find((h) => h.id === harnessId);
			const kindName = harness ? HARNESS_KINDS[harness.kind].name : "harness";
			// "waiting" wording surfaces the L2c case in toast / OS
			// banner so the user knows the agent wants something from
			// them — not that it finished. The ToastEntry's `state`
			// field flows into the existing toast component, which
			// renders it verbatim under the harness name.
			const stateLabel: "idle" | "exited" | "waiting" =
				to === "waiting" ? "waiting" : to === "idle" ? "idle" : "exited";
			// L5c — in-app toast. Fires when window IS focused but
			// the user isn't looking at the source harness (they're
			// in Skein, but in a different room or different tab).
			// Skipped when window is unfocused (OS notification
			// handles that case), when viewing the harness (badge
			// dot + tab color already tell the story), or when
			// disabled in Settings (L5e).
			if (notifyToastRef.current && isWindowFocused && !isViewedHarness && owningRoom && harness) {
				const entry: ToastEntry = {
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owningRoom.id,
					harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: stateLabel,
				};
				setToasts((prev) => [...prev, entry].slice(-TOAST_MAX_VISIBLE));
			}
			// OS notification — fire whenever the window isn't
			// focused, regardless of which harness was "viewed"
			// inside Skein. The user alt+tabbed away; they need
			// the OS-level signal to know to come back. When
			// focused, the badge update is already on screen and
			// an OS banner would just duplicate it. Also gated on
			// the per-surface Settings toggle (L5e).
			if (isWindowFocused) return;
			if (!notifyOsRef.current) return;
			if (notificationPermissionRef.current !== "granted") return;
			if (!owningRoom) return;
			// Serialized through the module-level chain (#84) so concurrent
			// transitions never call the plugin's `show` at the same time.
			// The helper also catches plugin-absent rejections (dev builds
			// skip it — see app/src-tauri/src/lib.rs).
			enqueueOsNotification("Skein", `${owningRoom.name} · ${kindName}: ${stateLabel}`);
		});
		return unsub;
	}, []);

	// D2f (#80) — graduated error treatment, steps 2+3 (handover §6).
	// api_error rows don't flow through harnessActivity (they're
	// harness_actions rows), so this dedicated listener feeds the
	// existing notification surfaces: an error-variant toast when the
	// error lands in a room the user isn't looking at, and a
	// pendingNotifications bump so the tab badge + status-bar urgent
	// segment persist until the room gets attention (the stream carries
	// no "resolved" signal — attention is the only clearing semantic).
	// A retry burst is one incident (real data: 4 rows in 11 s) — the
	// badge bumps once per window, and the toast updates in place while
	// it's still showing rather than stacking.
	useEffect(() => {
		const unlistenPromise = listen<HarnessAction>(ACTION_EVENT, (event) => {
			const a = event.payload;
			if (a.kind !== "api_error") return;
			// §6: the inline ApiErrorRow covers the active room; the toast
			// exists for errors the user can't currently see.
			if (a.roomId === activeRoomIdRef.current) return;
			const owningRoom = roomsRef.current.find((r) => r.id === a.roomId);
			const harness = owningRoom?.harnesses.find((h) => h.id === a.harnessId);
			if (!owningRoom || !harness) return;
			const now = Date.now();
			const last = lastApiErrorAtRef.current.get(a.harnessId) ?? 0;
			const newIncident = now - last > API_ERROR_INCIDENT_MS;
			lastApiErrorAtRef.current.set(a.harnessId, now);
			if (notifyBadgeRef.current && newIncident) {
				setRooms((prev) =>
					prev.map((r) => {
						if (!r.harnesses.some((h) => h.id === a.harnessId)) return r;
						return {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === a.harnessId
									? { ...h, pendingNotifications: (h.pendingNotifications ?? 0) + 1 }
									: h,
							),
						};
					}),
				);
			}
			if (!notifyToastRef.current || !windowFocusedRef.current) return;
			const detail = apiErrorToastText(parsePayload(a.payload));
			setToasts((prev) => {
				const i = prev.findIndex((t) => t.state === "error" && t.harnessId === a.harnessId);
				const existing = i === -1 ? undefined : prev[i];
				if (existing) {
					// Coalesce onto the live toast (same id) with fresh detail,
					// so a fast burst is one toast, not a stack. Retries that
					// outpace the 6s dismiss (529 backoff spaces them out:
					// ~0.5/1/2/4s and growing) let the toast lapse between
					// rows, so the next retry re-surfaces a fresh one — the
					// incident keeps re-announcing itself, which is fine; the
					// badge (one bump per incident) is the persistent signal.
					const next = [...prev];
					next[i] = { ...existing, detail };
					return next;
				}
				const entry: ToastEntry = {
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owningRoom.id,
					harnessId: a.harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: "error",
					detail,
				};
				return [...prev, entry].slice(-TOAST_MAX_VISIBLE);
			});
		});
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);

	// L6 — append every real phase transition to the sqlite event
	// log. Per-transition fire-and-forget; errors warn but don't
	// surface UX. The log feeds (eventually) the L7 cross-harness
	// activity feed; in the meantime the data exists for any
	// "since last visit" surface to build on. Epic #50 L6.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			const owningRoom = roomsRef.current.find((r) => r.harnesses.some((h) => h.id === harnessId));
			if (!owningRoom) {
				// Transition for a harness that's no longer in
				// state — e.g. exit firing after the room was
				// archived. Without a roomId we can't usefully log;
				// skip.
				return;
			}
			const activity = harnessActivity.get(harnessId);
			void invoke("db_record_harness_event", {
				harnessId,
				roomId: owningRoom.id,
				fromPhase: from,
				toPhase: to,
				timestampMs: Date.now(),
				hasUserInput: activity?.hasUserInput ?? false,
				// L7a (#73): per-transition attribution.
				// Identifies which strategy fired it (`l2a-idle`,
				// `l2b-pattern`, `l2c1-claude-end-turn`, etc.) for
				// the eventual L7c activity feed.
				source,
			}).catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] db_record_harness_event failed for ${harnessId}:`, msg);
			});
		});
		return unsub;
	}, []);

	// L5a — clear pending on view. Runs every time the (active room,
	// active harness of active room) tuple changes — covers tab
	// click, keyboard nav (Mod+1..9, Mod+Tab), command palette,
	// initial load. Only the harness that's now displayed gets
	// cleared; other harnesses in the same room keep their pending
	// counts so a multi-harness room only loses badges as the user
	// visits each tab.
	const displayedHarnessId = room?.activeHarnessId ?? null;
	useEffect(() => {
		if (!activeRoomId || !displayedHarnessId) return;
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== activeRoomId) return r;
				const target = r.harnesses.find((h) => h.id === displayedHarnessId);
				if (!target || (target.pendingNotifications ?? 0) === 0) return r;
				return {
					...r,
					harnesses: r.harnesses.map((h) =>
						h.id === displayedHarnessId ? { ...h, pendingNotifications: 0 } : h,
					),
				};
			}),
		);
	}, [activeRoomId, displayedHarnessId]);

	useEffect(() => {
		const cycleRoom = (delta: number) => {
			const list = activeRoomsRef.current;
			if (list.length === 0) return;
			const active = activeRoomIdRef.current;
			const idx = list.findIndex((r) => r.id === active);
			if (idx === -1) {
				const first = list[0];
				if (first) setActiveRoomId(first.id);
				return;
			}
			const nextIdx = (idx + delta + list.length) % list.length;
			const next = list[nextIdx];
			if (next) setActiveRoomId(next.id);
		};

		const cycleHarness = (delta: number) => {
			const list = activeRoomsRef.current;
			const active = activeRoomIdRef.current;
			const room = list.find((r) => r.id === active);
			if (!room || room.harnesses.length === 0) return;
			const idx = room.harnesses.findIndex((h) => h.id === room.activeHarnessId);
			const baseIdx = idx === -1 ? 0 : idx;
			const nextIdx = (baseIdx + delta + room.harnesses.length) % room.harnesses.length;
			const next = room.harnesses[nextIdx];
			if (next) switchHarnessInRoomRef.current(room.id, next.id);
		};

		const onKey = (e: KeyboardEvent) => {
			if (!isAppShortcut(e)) return;
			e.preventDefault();

			const active = activeRoomIdRef.current;

			// Mod+Shift combos
			if (e.shiftKey) {
				if (e.code === "KeyH") {
					if (active) addHarnessRef.current(active);
				} else if (e.code === "Tab" || e.code === "ArrowLeft") {
					cycleRoom(-1);
				} else if (e.code === "ArrowRight") {
					cycleRoom(1);
				}
				return;
			}

			// Mod-only combos
			switch (e.code) {
				case "Equal":
					setFontSize((s) => Math.min(FONT_MAX, s + 1));
					break;
				case "Minus":
					setFontSize((s) => Math.max(FONT_MIN, s - 1));
					break;
				case "KeyN":
					setShowNewRoom(true);
					break;
				case "KeyW":
					if (active) closeRoomRef.current(active);
					break;
				case "KeyK":
					setShowPalette(true);
					break;
				case "Comma":
					setShowSettings(true);
					break;
				case "Tab":
					cycleRoom(1);
					break;
				case "ArrowLeft":
					cycleHarness(-1);
					break;
				case "ArrowRight":
					cycleHarness(1);
					break;
				default:
					if (/^Digit[1-9]$/.test(e.code)) {
						const n = Number.parseInt(e.code.slice(5), 10) - 1;
						const target = activeRoomsRef.current[n];
						if (target) setActiveRoomId(target.id);
					}
			}
		};

		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [setFontSize]);

	// Phase 4: listen for the macOS app menu's Preferences… item.
	// lib.rs's on_menu_event emits skein://open-settings when the user
	// picks Skein → Preferences… from the menu bar; we open the same
	// modal as the cog icon and Mod+,.
	useEffect(() => {
		const promise = listen("skein://open-settings", () => setShowSettings(true));
		return () => {
			void promise.then((un) => un());
		};
	}, []);

	// All session ids any harness has already captured. captureOpencode
	// excludes these so a fresh capture can't claim someone else's id
	// when two opencode harnesses race in the same cwd.
	const claimedSessionIds = (): Set<string> =>
		new Set(
			roomsRef.current
				.flatMap((s) => s.harnesses.map((h) => h.sessionId))
				.filter((id): id is string => typeof id === "string"),
		);

	// Update one harness's sessionId after phase 2b's async capture
	// finds the new opencode row. Wrapped here so both creation paths
	// (pickHarness, createRoom) share the same setRooms shape.
	const setHarnessSessionId = (targetRoomId: string, harnessId: string, captured: string) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				return {
					...r,
					harnesses: r.harnesses.map((h) => {
						if (h.id !== harnessId) return h;
						// Idempotent: first-writer wins. Epic #50 L2c-2
						// races the SSE adapter's `session.created`
						// against the chapter-5 sqlite poll; whichever
						// fires first sets sessionId, the other becomes
						// a no-op. Without this guard, the sqlite poll
						// could find a *different* session (e.g. user
						// ran opencode in the same cwd from a shell
						// alongside) and overwrite the right id.
						if (h.sessionId) return h;
						return { ...h, sessionId: captured };
					}),
				};
			}),
		);
	};

	const pickHarness = async (kind: HarnessKind) => {
		const targetRoomId = showPicker;
		if (!targetRoomId) return;
		const targetRoom = rooms.find((r) => r.id === targetRoomId);
		if (!targetRoom) return;
		const id = newId("h");
		const cwd = targetRoom.cwd ?? defaultCwd;
		// Phase 2a: pre-allocate Claude's conversation id so the harness
		// resumes to *this* conversation on Skein restart — no picker.
		const sessionId = kind === "claude" ? crypto.randomUUID() : undefined;
		// Epic #50 L2c-2: allocate the opencode embedded-server port
		// *before* we set the cmd into state — LiveTerminal mounts the
		// PTY synchronously off the new harness record, so the port has
		// to be baked into the argv at that moment.
		let opencodePort: number | undefined;
		if (kind === "opencode") {
			try {
				opencodePort = await invoke<number>("pick_free_port");
				setOpencodePorts((prev) => {
					const m = new Map(prev);
					m.set(id, opencodePort as number);
					return m;
				});
			} catch (err) {
				console.warn("[skein] pick_free_port failed; falling back to L2a:", err);
			}
		}
		const cmd = cmdForKind(kind, defaultShell, sessionId, opencodePort);
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== targetRoomId) return r;
				const newH: Harness = {
					id,
					kind,
					name: `${HARNESS_KINDS[kind].label}-${r.harnesses.length + 1}`,
					status: "running",
					model: kind === "copilot" ? "gpt-5" : "sonnet-4.5",
					tokens: "0",
					live: true,
					cmd,
					cwd,
					...(sessionId ? { sessionId } : {}),
				};
				return { ...r, harnesses: [...r.harnesses, newH], activeHarnessId: id };
			}),
		);
		setShowPicker(null);
		// Phase 2b: kick off async capture for opencode harnesses. The
		// snapshot has to happen *before* opencode writes its session
		// row, which it doesn't do until LiveTerminal mounts and spawns
		// the binary — fine to fire-and-forget here, the React render
		// cycle keeps us ahead of the spawn.
		//
		// Epic #50 L2c-2: this is the sqlite-poll *fallback*. The
		// primary path is the SSE adapter capturing `session.created`
		// (wired in LiveTerminal). If the adapter beats this poll,
		// `setHarnessSessionId` is idempotent — the second write sees
		// `sessionId` already populated and the diff lookup in
		// `captureOpencodeSessionId` excludes it via `claimedSessionIds`.
		if (kind === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(targetRoomId, id, captured);
			});
		}
	};

	const createRoom = async ({ cwd, task, harness, branch }: CreateRoomArgs) => {
		const sid = newId("s");
		const hid = newId("h");
		// Phase 2a: pre-allocate Claude's conversation id (see pickHarness).
		const sessionId = harness === "claude" ? crypto.randomUUID() : undefined;
		// Epic #50 L2c-2: pre-allocate opencode's embedded-server port
		// before we bake the cmd into the harness record.
		let opencodePort: number | undefined;
		if (harness === "opencode") {
			try {
				opencodePort = await invoke<number>("pick_free_port");
				setOpencodePorts((prev) => {
					const m = new Map(prev);
					m.set(hid, opencodePort as number);
					return m;
				});
			} catch (err) {
				console.warn("[skein] pick_free_port failed; falling back to L2a:", err);
			}
		}
		// Display name from the trailing path component — `D:\code\skein`
		// → `skein`. Cosmetic; the actual cwd is what spawns use.
		const folderName =
			cwd
				.replace(/[\\/]+$/, "")
				.split(/[\\/]/)
				.pop() || cwd;
		// Repo / branch are only set for git-backed rooms (chapter 6
		// phase 3). For non-git rooms the tab subtext shows just the
		// folder name and LiveStatus is replaced by a placeholder.
		const newRoom: Room = {
			id: sid,
			name: `local · ${folderName}`,
			task,
			status: "running",
			badge: 0,
			cwd,
			...(branch ? { branch, repo: folderName } : {}),
			harnesses: [
				{
					id: hid,
					kind: harness,
					name: "main",
					status: "running",
					model: harness === "copilot" ? "gpt-5" : "sonnet-4.5",
					tokens: "0",
					live: true,
					cmd: cmdForKind(harness, defaultShell, sessionId, opencodePort),
					cwd,
					...(sessionId ? { sessionId } : {}),
				},
			],
			activeHarnessId: hid,
		};
		setRooms((prev) => [...prev, newRoom]);
		setActiveRoomId(sid);
		setShowNewRoom(false);
		// Phase 2b sqlite-poll fallback (see pickHarness comment for
		// the relationship with L2c-2's SSE capture).
		if (harness === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(sid, hid, captured);
			});
		}
	};

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
		onTheme: setTheme,
		onDensity: setDensity,
		onFontSize: setFontSize,
		notifyBadge,
		notifyToast,
		notifyUrgent,
		notifyOs,
		onNotifyBadge: setNotifyBadge,
		onNotifyToast: setNotifyToast,
		onNotifyUrgent: setNotifyUrgent,
		onNotifyOs: setNotifyOs,
		onClose: () => setShowSettings(false),
	};

	// Phase 4: items the command palette offers. Built every render
	// from current state — cheap at prototype scale (a few dozen rows).
	// Plain array, not useMemo: the cost of one filter+map per Ctrl+K
	// open is invisible, and useMemo here would mean tracking every
	// callback as a dep.
	const paletteItems: PaletteItem[] = [];
	for (const r of activeRooms) {
		paletteItems.push({
			id: `room:${r.id}`,
			label: `${r.name}`,
			hint: `room · ${r.branch}`,
			invoke: () => setActiveRoomId(r.id),
		});
	}
	for (const r of activeRooms) {
		for (const h of r.harnesses) {
			paletteItems.push({
				id: `harness:${h.id}`,
				label: `${HARNESS_KINDS[h.kind].name} · ${h.name}`,
				hint: `harness in ${r.name}`,
				invoke: () => {
					setActiveRoomId(r.id);
					setRooms((prev) =>
						prev.map((p) => (p.id === r.id ? { ...p, activeHarnessId: h.id } : p)),
					);
				},
			});
		}
	}
	paletteItems.push({
		id: "cmd:new-room",
		label: "New room",
		hint: `${modLabel} N`,
		invoke: () => setShowNewRoom(true),
	});
	if (archivedRooms.length > 0) {
		paletteItems.push({
			id: "cmd:reopen-room",
			label: `Reopen room… (${archivedRooms.length})`,
			invoke: () => setShowReopen(true),
		});
	}
	if (activeRoomId) {
		paletteItems.push({
			id: "cmd:add-harness",
			label: "Add harness to active room",
			hint: `${modLabel} ⇧ H`,
			invoke: () => addHarness(activeRoomId),
		});
		paletteItems.push({
			id: "cmd:close-room",
			label: "Close active room",
			hint: `${modLabel} W`,
			invoke: () => closeRoom(activeRoomId),
		});
	}
	paletteItems.push({
		id: "cmd:toggle-theme",
		label: `Toggle theme (currently ${theme})`,
		invoke: () => setTheme(theme === "dark" ? "light" : "dark"),
	});

	// L5c — toast stack rendered identically in both branches below
	// (empty state and the normal app layout). Floats above
	// everything via `position: fixed`; pointer-events on the
	// container is `none` so it doesn't catch clicks on the
	// underlying app, while each toast re-enables them.
	const toastStack = toasts.length > 0 && (
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
	);

	// Empty state — no *active* rooms. Archived rooms still in the list
	// show via the reopen modal (linked from the empty state too).
	if (activeRooms.length === 0) {
		return (
			<div
				className={`sk-app sk-${theme} density-${density}`}
				data-platform={isMac ? "mac" : "other"}
			>
				<Titlebar {...titlebarProps} />
				<EmptyState
					onNew={() => setShowNewRoom(true)}
					archivedCount={archivedRooms.length}
					onReopen={() => setShowReopen(true)}
				/>
				{showNewRoom && (
					<NewRoomDialog
						defaultCwd={defaultCwd}
						onCommit={createRoom}
						onCancel={() => setShowNewRoom(false)}
					/>
				)}
				{showPalette && (
					<CommandPalette items={paletteItems} onClose={() => setShowPalette(false)} />
				)}
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
				{toastStack}
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
		>
			<Titlebar {...titlebarProps} />

			<div className="sk-tabstrip">
				{activeRooms.map((r) => {
					const isDraggedRoom = drag?.kind === "room" && drag.id === r.id;
					const dropSide =
						dropTarget?.kind === "room" && dropTarget.id === r.id ? dropTarget.side : null;
					return (
						<LiveRoomTab
							key={r.id}
							r={r}
							active={r.id === activeRoomId}
							onClick={() => switchRoom(r.id)}
							onClose={() => closeRoom(r.id)}
							draggable
							dragging={isDraggedRoom}
							dropSide={dropSide}
							onDragStart={(e) => handleRoomDragStart(e, r.id)}
							onDragOver={(e) => handleRoomDragOver(e, r.id)}
							onDrop={(e) => handleRoomDrop(e, r.id)}
							onDragEnd={handleDragEnd}
						/>
					);
				})}
				<div className="sk-tab-newbtn" onClick={() => setShowNewRoom(true)} title="New room">
					+
				</div>
			</div>

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
						<HarnessColumn
							room={r}
							fontSize={fontSize}
							defaultShell={defaultShell}
							showPicker={showPicker === r.id}
							roomActive={r.id === activeRoomId}
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
								onDragStart: handleHarnessDragStart,
								onDragOver: handleHarnessDragOver,
								onDrop: handleHarnessDrop,
								onDragEnd: handleDragEnd,
							}}
							onPick={pickHarness}
							onAddHarness={addHarness}
							onSwitchHarness={switchHarnessInRoom}
							onCloseHarness={closeHarness}
							onHarnessCmdChange={updateHarnessCmd}
							opencodePorts={opencodePorts}
							onOpencodeSessionCaptured={(harnessId, sid) =>
								setHarnessSessionId(r.id, harnessId, sid)
							}
						/>
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
						{/* The right pane is the Live Context card stack (issue #80):
						    Diff / Plan / Activity, sourced from harness_actions. It
						    also keeps the status-bar branch live via a lightweight
						    git watcher (issue #18), the role LiveStatus used to own. */}
						{r.cwd ? (
							<LiveContext
								roomId={r.id}
								cwd={r.cwd}
								harnesses={r.harnesses}
								focusedHarnessId={r.activeHarnessId}
								visible={r.id === activeRoomId}
								showTurnCosts={showTurnCosts}
								onToggleTurnCosts={handleToggleTurnCosts}
								onBranchChange={handleBranchChange}
							/>
						) : null}
					</div>
				))}
			/>

			<div className="sk-statusbar">
				<span className="seg">
					<HChip kind={activeHarness.kind} size={10} />
					<span>{HARNESS_KINDS[activeHarness.kind].name}</span>
				</span>
				<LiveStatusBarChip harness={activeHarness} />
				{(() => {
					// Prefer the live branch (updated on every watcher tick) but
					// fall back to room.branch on first render before LiveStatus
					// has had a chance to refresh. Issue #18.
					const live = liveBranches[room.id];
					const branch = live === undefined ? room.branch : (live ?? undefined);
					if (!branch) return null;
					const drifted =
						live !== undefined && live !== null && room.branch && live !== room.branch;
					return (
						<span
							className="seg"
							title={drifted ? `worktree branch was ${room.branch}` : undefined}
						>
							{branch}
							{drifted && <span style={{ color: "var(--warn)", marginLeft: 4 }}>•</span>}
						</span>
					);
				})()}
				{room.cwd && (
					<span className="seg sk-statusbar-cwd" title={room.cwd}>
						{room.cwd}
					</span>
				)}
				<span className="spacer" />
				{notifyUrgent &&
					(() => {
						// L5d — urgent segment. Scan active (non-archived,
						// non-active) rooms for any pending notifications;
						// surface the room with the biggest backlog so
						// the user has a one-click jump to whatever
						// needs attention most. Ties broken by room
						// order. Hidden when no room has anything pending,
						// or when the surface is disabled in Settings (L5e).
						let target: { room: Room; total: number } | null = null;
						for (const r of activeRooms) {
							if (r.id === activeRoomId) continue;
							const total = r.harnesses.reduce((acc, h) => acc + (h.pendingNotifications ?? 0), 0);
							if (total === 0) continue;
							if (!target || total > target.total) target = { room: r, total };
						}
						if (!target) return null;
						return (
							<span
								className="seg sk-statusbar-urgent"
								title={`Jump to ${target.room.name}`}
								onClick={() => {
									// Land on the harness that drove the room to the top —
									// most pending, ties broken by harness order (#65).
									const winner =
										[...target.room.harnesses]
											.filter((h) => (h.pendingNotifications ?? 0) > 0)
											.sort(
												(a, b) => (b.pendingNotifications ?? 0) - (a.pendingNotifications ?? 0),
											)[0] ?? target.room.harnesses[0];
									if (winner) jumpToHarness(target.room.id, winner.id);
									else switchRoom(target.room.id);
								}}
							>
								<span className="dot-tiny st-waiting" />
								{target.room.name}
								<span className="sk-statusbar-urgent-count">{target.total}</span>
							</span>
						);
					})()}
			</div>

			{showNewRoom && (
				<NewRoomDialog
					defaultCwd={defaultCwd}
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
			{toastStack}
		</div>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
