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
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { CommandPalette, type PaletteItem } from "./CommandPalette.tsx";
import { LiveStatus } from "./LiveStatus.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import { ReopenRoomModal } from "./ReopenRoomModal.tsx";
import { SettingsModal } from "./SettingsModal.tsx";
import { Splitter } from "./Splitter.tsx";
import { HChip, HarnessPicker, HarnessTab, RoomTab, StatusDot } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { usePersistedState } from "./prefs.ts";
import { isAppShortcut, isMac, modLabel } from "./shortcuts.ts";
import type { Density, Harness, HarnessKind, Room, Theme } from "./types.ts";

// ── Harness body ───────────────────────────────────────────────────

interface HarnessBodyProps {
	harness: Harness;
	fontSize: number;
	defaultShell: string[];
	visible: boolean;
	onCmdChange: (cmd: string[]) => void;
}

const HarnessBody = ({
	harness,
	fontSize,
	defaultShell,
	visible,
	onCmdChange,
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

interface HarnessColumnProps {
	room: Room;
	fontSize: number;
	defaultShell: string[];
	showPicker: boolean;
	// True iff this column's room is the active room. Combined with
	// `showPicker` and per-harness activeness, it tells each
	// LiveTerminal whether it should hold keyboard focus. Issue #22.
	roomActive: boolean;
	onPick: (kind: HarnessKind) => void;
	onAddHarness: (roomId: string) => void;
	onSwitchHarness: (roomId: string, harnessId: string) => void;
	onCloseHarness: (roomId: string, harnessId: string) => void;
	onHarnessCmdChange: (roomId: string, harnessId: string, cmd: string[]) => void;
}

const HarnessColumn = ({
	room,
	fontSize,
	defaultShell,
	showPicker,
	roomActive,
	onPick,
	onAddHarness,
	onSwitchHarness,
	onCloseHarness,
	onHarnessCmdChange,
}: HarnessColumnProps) => (
	<div className="sk-harness-col">
		<div className="sk-harness-tabs">
			{room.harnesses.map((h) => (
				<HarnessTab
					key={h.id}
					h={h}
					active={h.id === room.activeHarnessId}
					closable={room.harnesses.length > 1}
					onClick={() => onSwitchHarness(room.id, h.id)}
					onClose={() => onCloseHarness(room.id, h.id)}
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
const cmdForKind = (kind: HarnessKind, fallbackShell: string[], sessionId?: string): string[] => {
	switch (kind) {
		case "claude":
			return sessionId ? ["claude", "--session-id", sessionId] : ["claude"];
		case "opencode":
			return ["opencode"];
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
const resumeCmd = (h: Harness): string[] => {
	const cmd = h.cmd ?? [];
	if (h.kind === "claude") {
		const isFreshClaude =
			(cmd.length === 1 && cmd[0] === "claude") ||
			(cmd.length === 3 && cmd[0] === "claude" && cmd[1] === "--session-id");
		if (isFreshClaude) {
			return h.sessionId ? ["claude", "--resume", h.sessionId] : ["claude", "--resume"];
		}
	}
	if (h.kind === "opencode" && cmd.length === 1 && cmd[0] === "opencode") {
		return h.sessionId ? ["opencode", "--session", h.sessionId] : ["opencode", "--continue"];
	}
	return cmd;
};

export default function App() {
	const [theme, setTheme] = usePersistedState<Theme>("theme", "dark");
	const [density, setDensity] = usePersistedState<Density>("density", "regular");
	const [fontSize, setFontSize] = usePersistedState<number>("fontSize", FONT_DEFAULT);
	// Width of the harness column in px. Right pane absorbs the remainder
	// via flex:1. Splitter clamps against window size at drag time.
	const [harnessColWidth, setHarnessColWidth] = usePersistedState<number>("harnessColWidth", 640);

	const [rooms, setRooms] = useState<Room[]>([]);
	const [activeRoomId, setActiveRoomId] = useState<string>("");
	// Live HEAD branch per room, populated by LiveStatus on every watcher
	// tick. `room.branch` is the *creation* branch (worktree identity);
	// this is what's actually checked out right now. The status bar reads
	// from here first so a `git checkout` inside a harness is visible.
	// Issue #18.
	const [liveBranches, setLiveBranches] = useState<Record<string, string | null>>({});
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
					// Rewrite each harness's cmd to its resume form before
					// mounting, so the PTY spawn re-attaches to the prior
					// conversation instead of starting fresh.
					const withResume = verified.map((r) => ({
						...r,
						harnesses: r.harnesses.map((h) => (h.cmd ? { ...h, cmd: resumeCmd(h) } : h)),
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

	const switchHarnessInRoom = (roomId: string, harnessId: string) => {
		setRooms((prev) =>
			prev.map((r) => (r.id === roomId ? { ...r, activeHarnessId: harnessId } : r)),
		);
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
			prev.map((r) =>
				r.id === targetRoomId
					? {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === harnessId ? { ...h, sessionId: captured } : h,
							),
						}
					: r,
			),
		);
	};

	const pickHarness = (kind: HarnessKind) => {
		const targetRoomId = showPicker;
		if (!targetRoomId) return;
		const targetRoom = rooms.find((r) => r.id === targetRoomId);
		if (!targetRoom) return;
		const id = newId("h");
		const cwd = targetRoom.cwd ?? defaultCwd;
		// Phase 2a: pre-allocate Claude's conversation id so the harness
		// resumes to *this* conversation on Skein restart — no picker.
		const sessionId = kind === "claude" ? crypto.randomUUID() : undefined;
		const cmd = cmdForKind(kind, defaultShell, sessionId);
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
		if (kind === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(targetRoomId, id, captured);
			});
		}
	};

	const createRoom = ({ cwd, task, harness, branch }: CreateRoomArgs) => {
		const sid = newId("s");
		const hid = newId("h");
		// Phase 2a: pre-allocate Claude's conversation id (see pickHarness).
		const sessionId = harness === "claude" ? crypto.randomUUID() : undefined;
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
					cmd: cmdForKind(harness, defaultShell, sessionId),
					cwd,
					...(sessionId ? { sessionId } : {}),
				},
			],
			activeHarnessId: hid,
		};
		setRooms((prev) => [...prev, newRoom]);
		setActiveRoomId(sid);
		setShowNewRoom(false);
		// Phase 2b: same pattern as pickHarness — async capture for
		// opencode's auto-assigned session id.
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
						onClose={() => setShowReopen(false)}
					/>
				)}
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
				{activeRooms.map((r) => (
					<RoomTab
						key={r.id}
						r={r}
						active={r.id === activeRoomId}
						onClick={() => switchRoom(r.id)}
						onClose={() => closeRoom(r.id)}
					/>
				))}
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
							onPick={pickHarness}
							onAddHarness={addHarness}
							onSwitchHarness={switchHarnessInRoom}
							onCloseHarness={closeHarness}
							onHarnessCmdChange={updateHarnessCmd}
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
						{/* LiveStatus owns the git-vs-not decision now (issue #6). It
						    starts a watcher even on non-git cwds and self-promotes when
						    a `.git` dir appears, so the room stops being frozen at
						    creation time. */}
						{r.cwd ? (
							<LiveStatus
								cwd={r.cwd}
								onBranchChange={(b) =>
									setLiveBranches((prev) => (prev[r.id] === b ? prev : { ...prev, [r.id]: b }))
								}
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
				<span className="seg">
					<span className={`dot-tiny st-${activeHarness.status}`} />
					{activeHarness.status}
				</span>
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
					onClose={() => setShowReopen(false)}
				/>
			)}
		</div>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
