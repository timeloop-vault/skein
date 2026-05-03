// Skein interactive prototype — single React tree.
//
// Mental model:
//   - Tabs along the top are sessions (workspace = repo + branch + task).
//   - Each session owns N harnesses (Claude Code, opencode, gh copilot,
//     or a built-in shell). All harnesses in a session share the same
//     worktree.
//   - The right pane belongs to the session, not the harness — so when
//     you switch agents inside the same workspace, the diff and status
//     stay put.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { type CSSProperties, useEffect, useMemo, useRef, useState } from "react";
import { CommandPalette, type PaletteItem } from "./CommandPalette.tsx";
import { LiveStatus } from "./LiveStatus.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import { SettingsModal } from "./SettingsModal.tsx";
import { Splitter } from "./Splitter.tsx";
import { HChip, HarnessPicker, HarnessTab, SessionTab, StatusDot } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { usePersistedState } from "./prefs.ts";
import { isAppShortcut, isMac, modLabel } from "./shortcuts.ts";
import type { Density, Harness, HarnessKind, Session, Theme } from "./types.ts";

// ── Harness body ───────────────────────────────────────────────────

interface HarnessBodyProps {
	harness: Harness;
	fontSize: number;
	defaultShell: string[];
	onCmdChange: (cmd: string[]) => void;
}

const HarnessBody = ({ harness, fontSize, defaultShell, onCmdChange }: HarnessBodyProps) => {
	if (harness.cmd && harness.cwd !== undefined) {
		return (
			<LiveTerminal
				cmd={harness.cmd}
				cwd={harness.cwd}
				mountKey={harness.id}
				fontSize={fontSize}
				defaultShell={defaultShell}
				onCmdChange={onCmdChange}
			/>
		);
	}
	return null;
};

// ── Harness column (per session) ───────────────────────────────────
// Phase 3: every session's column stays mounted at once; the App-level
// renderer toggles visibility with display:none. PTYs survive tab
// switches because LiveTerminal's effect is keyed on mountKey only.

interface HarnessColumnProps {
	session: Session;
	fontSize: number;
	defaultShell: string[];
	showPicker: boolean;
	onPick: (kind: HarnessKind) => void;
	onAddHarness: (sessionId: string) => void;
	onSwitchHarness: (sessionId: string, harnessId: string) => void;
	onCloseHarness: (sessionId: string, harnessId: string) => void;
	onHarnessCmdChange: (sessionId: string, harnessId: string, cmd: string[]) => void;
}

const HarnessColumn = ({
	session,
	fontSize,
	defaultShell,
	showPicker,
	onPick,
	onAddHarness,
	onSwitchHarness,
	onCloseHarness,
	onHarnessCmdChange,
}: HarnessColumnProps) => (
	<div className="sk-harness-col">
		<div className="sk-harness-tabs">
			{session.harnesses.map((h) => (
				<HarnessTab
					key={h.id}
					h={h}
					active={h.id === session.activeHarnessId}
					closable={session.harnesses.length > 1}
					onClick={() => onSwitchHarness(session.id, h.id)}
					onClose={() => onCloseHarness(session.id, h.id)}
				/>
			))}
			<div className="sk-harness-add" onClick={() => onAddHarness(session.id)}>
				+ harness
			</div>
			<div className="sk-harness-meta">
				<span>
					{session.repo} · {session.branch}
				</span>
			</div>
		</div>

		{showPicker ? (
			<HarnessPicker onPick={onPick} />
		) : (
			// Mount every harness in this session at once; hide the
			// inactive ones via display:none so xterm scrollback,
			// cursor position, and PTY state survive harness-tab
			// switches inside the session.
			session.harnesses.map((h) => (
				<div
					key={h.id}
					style={{
						display: h.id === session.activeHarnessId ? "flex" : "none",
						flexDirection: "column",
						flex: 1,
						minHeight: 0,
					}}
				>
					<HarnessBody
						harness={h}
						fontSize={fontSize}
						defaultShell={defaultShell}
						onCmdChange={(newCmd) => onHarnessCmdChange(session.id, h.id, newCmd)}
					/>
				</div>
			))
		)}
	</div>
);

// xterm font size range. Outside this band the terminal looks either
// unreadable (sub-12) or comically large (above 18) on a 1320x820 window.
const FONT_MIN = 12;
const FONT_MAX = 18;
const FONT_DEFAULT = 13;

// UI scale (zoom on .sk-app) — separate from the terminal font so a
// large monitor can have readable chrome without a giant terminal grid.
// 0.85 still legible on 1366×768; 1.4 is comfortable on 4K.
const UI_SCALE_MIN = 0.85;
const UI_SCALE_MAX = 1.4;
const UI_SCALE_STEP = 0.05;
const UI_SCALE_DEFAULT = 1.0;

// ── New session dialog ─────────────────────────────────────────────
// The picked folder becomes the session's cwd; every harness in the
// session spawns into it. "New worktree" mode resolves to a fresh
// libgit2 worktree path; "Current branch" mode uses the picked path
// as-is.

interface BranchInfoDto {
	name: string;
	isHead: boolean;
}

// What the dialog hands back. The cwd is already the *real* directory
// the spawn should land in — for "New worktree" mode the dialog has
// already called git_add_worktree and resolved the worktree path; for
// "Current branch" mode it's just the picked repo path.
interface CreateSessionArgs {
	cwd: string;
	task: string;
	harness: HarnessKind;
	branch: string;
	branchMode: "worktree" | "current";
}

type RepoStatus =
	| { kind: "empty" }
	| { kind: "checking" }
	| { kind: "valid"; branches: BranchInfoDto[]; head: string | null }
	| { kind: "not-a-repo" };

const NewSessionDialog = ({
	defaultCwd,
	onCommit,
	onCancel,
}: {
	defaultCwd: string;
	onCommit: (args: CreateSessionArgs) => void;
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

	const valid = repoStatus.kind === "valid";
	const canCreate =
		task.trim().length > 0 && valid && !busy && (branchMode === "current" || baseBranch.length > 0);

	const browse = async () => {
		const start = cwd || defaultCwd;
		const picked = await openDialog({
			directory: true,
			multiple: false,
			title: "Pick a folder for this session",
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
					branchMode,
				});
			} else {
				onCommit({
					cwd,
					task: task.trim(),
					harness,
					branch: repoStatus.kind === "valid" ? (repoStatus.head ?? "HEAD") : "HEAD",
					branchMode,
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
				return <span style={{ color: "var(--err)" }}>⚠ not a git repository</span>;
		}
	})();

	return (
		<div className="sk-modal-bg" onClick={onCancel}>
			<div className="sk-modal" onClick={(e) => e.stopPropagation()}>
				<div className="sk-modal-head">
					<h2>New session</h2>
					<div className="sub">
						A session is a folder + task. You can add more harnesses inside.
					</div>
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

					{valid && (
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
						{busy ? "Creating…" : "Create session"}
					</button>
				</div>
			</div>
		</div>
	);
};

// ── Empty state ────────────────────────────────────────────────────

const EmptyState = ({ onNew }: { onNew: () => void }) => (
	<div className="sk-empty">
		<div className="glyph">⊜</div>
		<h1>No sessions yet</h1>
		<div className="lede">
			A session pins a repo and a task. Open as many harnesses inside as you want — Claude Code and
			opencode on the same worktree, two Copilot runs on a fix, whatever shape the work takes.
		</div>
		<button className="start-btn" onClick={onNew}>
			Create your first session
		</button>
		<div className="hint-list">
			<div className="row">
				<span className="kbd">{modLabel} N</span>
				<span>New session</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} ⇧ H</span>
				<span>Add harness to current session</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} Tab</span>
				<span>Next session (⇧ for previous, 1-9 for nth)</span>
			</div>
			<div className="row">
				<span className="kbd">{modLabel} W</span>
				<span>Close active session</span>
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
	activeSessionLabel: string | null;
	onOpenSettings: () => void;
}

const Titlebar = ({ activeSessionLabel, onOpenSettings }: TitlebarProps) => (
	<div className="sk-titlebar" data-tauri-drag-region>
		<span className="sk-app-name">
			<span className="dot">●</span> skein
		</span>
		{activeSessionLabel && <span className="sk-titlebar-session">{activeSessionLabel}</span>}
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
	const [uiScale, setUiScale] = usePersistedState<number>("uiScale", UI_SCALE_DEFAULT);
	// Width of the harness column in px. Right pane absorbs the remainder
	// via flex:1. Splitter clamps against window size at drag time.
	const [harnessColWidth, setHarnessColWidth] = usePersistedState<number>("harnessColWidth", 640);

	const [sessions, setSessions] = useState<Session[]>([]);
	const [activeSessionId, setActiveSessionId] = useState<string>("");
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [showNewSession, setShowNewSession] = useState(false);
	const [showPalette, setShowPalette] = useState(false);
	const [showSettings, setShowSettings] = useState(false);

	// Phase 1: pull platform defaults once at boot. New harnesses spawn
	// into these until Phase 4 wires real worktrees / per-session cwd.
	const [defaultShell, setDefaultShell] = useState<string[]>([]);
	const [defaultCwd, setDefaultCwd] = useState<string>("");
	useEffect(() => {
		void invoke<string[]>("default_shell").then(setDefaultShell);
		void invoke<string>("default_cwd").then(setDefaultCwd);
	}, []);

	// Phase 3: hydrate sessions from sqlite on boot. Until that round-trips,
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

		invoke<Session[]>("db_load_sessions")
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
					const withResume = verified.map((s) => ({
						...s,
						harnesses: s.harnesses.map((h) => (h.cmd ? { ...h, cmd: resumeCmd(h) } : h)),
					}));
					setSessions(withResume);
					const first = withResume[0];
					if (first) setActiveSessionId(first.id);
				}
				setLoaded(true);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] db_load_sessions failed:", msg);
				setLoaded(true);
			});
	}, []);

	// Phase 3: any time `sessions` changes after the initial load, mirror
	// the new state to sqlite. Wipe-and-insert is fine at prototype scale.
	useEffect(() => {
		if (!loaded) return;
		void invoke("db_save_sessions", { sessions }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.error("[skein] db_save_sessions failed:", msg);
		});
	}, [sessions, loaded]);

	const session = useMemo(
		() => sessions.find((s) => s.id === activeSessionId),
		[sessions, activeSessionId],
	);
	const activeHarness = session?.harnesses.find((h) => h.id === session.activeHarnessId);

	const switchSession = (id: string) => {
		setActiveSessionId(id);
	};

	const closeSession = (id: string) => {
		// Confirm before delete — sessions can hold a lot of state and
		// the prototype has no undo. window.confirm is fine for v0.
		if (!window.confirm("Close this session? Any running harnesses will be killed.")) {
			return;
		}
		setSessions((prev) => {
			const remaining = prev.filter((s) => s.id !== id);
			if (id === activeSessionId) {
				const first = remaining[0];
				setActiveSessionId(first ? first.id : "");
			}
			return remaining;
		});
	};

	const switchHarnessInSession = (sessionId: string, harnessId: string) => {
		setSessions((prev) =>
			prev.map((s) => (s.id === sessionId ? { ...s, activeHarnessId: harnessId } : s)),
		);
	};

	const closeHarness = (sessionId: string, harnessId: string) => {
		setSessions((prev) =>
			prev.map((s) => {
				if (s.id !== sessionId) return s;
				const remaining = s.harnesses.filter((h) => h.id !== harnessId);
				if (remaining.length === 0) return s;
				const first = remaining[0];
				if (!first) return s;
				return { ...s, harnesses: remaining, activeHarnessId: first.id };
			}),
		);
	};

	// Phase 4: when a harness's child exits and the user picks the
	// shell-fallback path, LiveTerminal calls this so the new cmd
	// persists to the DB and a Skein restart re-spawns the shell.
	const updateHarnessCmd = (sessionId: string, harnessId: string, cmd: string[]) => {
		setSessions((prev) =>
			prev.map((s) =>
				s.id === sessionId
					? { ...s, harnesses: s.harnesses.map((h) => (h.id === harnessId ? { ...h, cmd } : h)) }
					: s,
			),
		);
	};

	const addHarness = (sessionId: string) => setShowPicker(sessionId);

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
	const closeSessionRef = useRef(closeSession);
	closeSessionRef.current = closeSession;
	const sessionsRef = useRef(sessions);
	sessionsRef.current = sessions;
	const activeSessionIdRef = useRef(activeSessionId);
	activeSessionIdRef.current = activeSessionId;

	useEffect(() => {
		const cycleSession = (delta: number) => {
			const list = sessionsRef.current;
			if (list.length === 0) return;
			const active = activeSessionIdRef.current;
			const idx = list.findIndex((s) => s.id === active);
			if (idx === -1) {
				const first = list[0];
				if (first) setActiveSessionId(first.id);
				return;
			}
			const nextIdx = (idx + delta + list.length) % list.length;
			const next = list[nextIdx];
			if (next) setActiveSessionId(next.id);
		};

		const onKey = (e: KeyboardEvent) => {
			if (!isAppShortcut(e)) return;
			e.preventDefault();

			const active = activeSessionIdRef.current;

			// Mod+Shift combos
			if (e.shiftKey) {
				if (e.code === "KeyH") {
					if (active) addHarnessRef.current(active);
				} else if (e.code === "Tab") {
					cycleSession(-1);
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
					setShowNewSession(true);
					break;
				case "KeyW":
					if (active) closeSessionRef.current(active);
					break;
				case "KeyK":
					setShowPalette(true);
					break;
				case "Comma":
					setShowSettings(true);
					break;
				case "Tab":
					cycleSession(1);
					break;
				default:
					if (/^Digit[1-9]$/.test(e.code)) {
						const n = Number.parseInt(e.code.slice(5), 10) - 1;
						const target = sessionsRef.current[n];
						if (target) setActiveSessionId(target.id);
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
			sessionsRef.current
				.flatMap((s) => s.harnesses.map((h) => h.sessionId))
				.filter((id): id is string => typeof id === "string"),
		);

	// Update one harness's sessionId after phase 2b's async capture
	// finds the new opencode row. Wrapped here so both creation paths
	// (pickHarness, createSession) share the same setSessions shape.
	const setHarnessSessionId = (targetSessionId: string, harnessId: string, captured: string) => {
		setSessions((prev) =>
			prev.map((s) =>
				s.id === targetSessionId
					? {
							...s,
							harnesses: s.harnesses.map((h) =>
								h.id === harnessId ? { ...h, sessionId: captured } : h,
							),
						}
					: s,
			),
		);
	};

	const pickHarness = (kind: HarnessKind) => {
		const targetSessionId = showPicker;
		if (!targetSessionId) return;
		const targetSession = sessions.find((s) => s.id === targetSessionId);
		if (!targetSession) return;
		const id = newId("h");
		const cwd = targetSession.cwd ?? defaultCwd;
		// Phase 2a: pre-allocate Claude's conversation id so the harness
		// resumes to *this* session on Skein restart — no picker.
		const sessionId = kind === "claude" ? crypto.randomUUID() : undefined;
		const cmd = cmdForKind(kind, defaultShell, sessionId);
		setSessions((prev) =>
			prev.map((s) => {
				if (s.id !== targetSessionId) return s;
				const newH: Harness = {
					id,
					kind,
					name: `${HARNESS_KINDS[kind].label}-${s.harnesses.length + 1}`,
					status: "running",
					model: kind === "copilot" ? "gpt-5" : "sonnet-4.5",
					tokens: "0",
					live: true,
					cmd,
					cwd,
					...(sessionId ? { sessionId } : {}),
				};
				return { ...s, harnesses: [...s.harnesses, newH], activeHarnessId: id };
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
				setHarnessSessionId(targetSessionId, id, captured);
			});
		}
	};

	const createSession = ({ cwd, task, harness, branch }: CreateSessionArgs) => {
		const sid = newId("s");
		const hid = newId("h");
		// Phase 2a: pre-allocate Claude's conversation id (see pickHarness).
		const sessionId = harness === "claude" ? crypto.randomUUID() : undefined;
		// "Repo" name from the trailing path component — `D:\code\skein` →
		// `skein`. Cosmetic; the actual cwd is what spawns use.
		const repoName =
			cwd
				.replace(/[\\/]+$/, "")
				.split(/[\\/]/)
				.pop() || cwd;
		const newSession: Session = {
			id: sid,
			name: `local · ${repoName}`,
			branch,
			repo: repoName,
			task,
			status: "running",
			badge: 0,
			cwd,
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
		setSessions((prev) => [...prev, newSession]);
		setActiveSessionId(sid);
		setShowNewSession(false);
		// Phase 2b: same pattern as pickHarness — async capture for
		// opencode's auto-assigned session id.
		if (harness === "opencode") {
			void captureOpencodeSessionId(cwd, claimedSessionIds, (captured) => {
				setHarnessSessionId(sid, hid, captured);
			});
		}
	};

	const titlebarProps: TitlebarProps = {
		activeSessionLabel: session ? session.name : null,
		onOpenSettings: () => setShowSettings(true),
	};

	const settingsProps = {
		theme,
		density,
		fontSize,
		uiScale,
		fontMin: FONT_MIN,
		fontMax: FONT_MAX,
		uiScaleMin: UI_SCALE_MIN,
		uiScaleMax: UI_SCALE_MAX,
		uiScaleStep: UI_SCALE_STEP,
		onTheme: setTheme,
		onDensity: setDensity,
		onFontSize: setFontSize,
		onUiScale: setUiScale,
		onClose: () => setShowSettings(false),
	};

	// Phase 4: items the command palette offers. Built every render
	// from current state — cheap at prototype scale (a few dozen rows).
	// Plain array, not useMemo: the cost of one filter+map per Ctrl+K
	// open is invisible, and useMemo here would mean tracking every
	// callback as a dep.
	const paletteItems: PaletteItem[] = [];
	for (const s of sessions) {
		paletteItems.push({
			id: `session:${s.id}`,
			label: `${s.name}`,
			hint: `session · ${s.branch}`,
			invoke: () => setActiveSessionId(s.id),
		});
	}
	for (const s of sessions) {
		for (const h of s.harnesses) {
			paletteItems.push({
				id: `harness:${h.id}`,
				label: `${HARNESS_KINDS[h.kind].name} · ${h.name}`,
				hint: `harness in ${s.name}`,
				invoke: () => {
					setActiveSessionId(s.id);
					setSessions((prev) =>
						prev.map((p) => (p.id === s.id ? { ...p, activeHarnessId: h.id } : p)),
					);
				},
			});
		}
	}
	paletteItems.push({
		id: "cmd:new-session",
		label: "New session",
		hint: `${modLabel} N`,
		invoke: () => setShowNewSession(true),
	});
	if (activeSessionId) {
		paletteItems.push({
			id: "cmd:add-harness",
			label: "Add harness to active session",
			hint: `${modLabel} ⇧ H`,
			invoke: () => addHarness(activeSessionId),
		});
		paletteItems.push({
			id: "cmd:close-session",
			label: "Close active session",
			hint: `${modLabel} W`,
			invoke: () => closeSession(activeSessionId),
		});
	}
	paletteItems.push({
		id: "cmd:toggle-theme",
		label: `Toggle theme (currently ${theme})`,
		invoke: () => setTheme(theme === "dark" ? "light" : "dark"),
	});

	// CSS `zoom` on the root scales the entire chrome uniformly.
	// xterm's container scales with it, but the terminal font is pinned
	// by `fontSize` (xterm option), so the cell size in CSS pixels stays
	// the same — the user just sees a bigger or smaller grid (more rows
	// at lower scale, fewer at higher). That's what we want here:
	// independent control over chrome density and terminal density.
	const appStyle: CSSProperties = { zoom: uiScale };

	// Empty state — no sessions at all.
	if (sessions.length === 0) {
		return (
			<div
				className={`sk-app sk-${theme} density-${density}`}
				data-platform={isMac ? "mac" : "other"}
				style={appStyle}
			>
				<Titlebar {...titlebarProps} />
				<EmptyState onNew={() => setShowNewSession(true)} />
				{showNewSession && (
					<NewSessionDialog
						defaultCwd={defaultCwd}
						onCommit={createSession}
						onCancel={() => setShowNewSession(false)}
					/>
				)}
				{showPalette && (
					<CommandPalette items={paletteItems} onClose={() => setShowPalette(false)} />
				)}
				{showSettings && <SettingsModal {...settingsProps} />}
			</div>
		);
	}

	if (!session || !activeHarness) {
		// Shouldn't happen in practice: sessions is non-empty above.
		return null;
	}

	return (
		<div
			className={`sk-app sk-${theme} density-${density}`}
			data-platform={isMac ? "mac" : "other"}
			style={appStyle}
		>
			<Titlebar {...titlebarProps} />

			<div className="sk-tabstrip">
				{sessions.map((s) => (
					<SessionTab
						key={s.id}
						s={s}
						active={s.id === activeSessionId}
						onClick={() => switchSession(s.id)}
						onClose={() => closeSession(s.id)}
					/>
				))}
				<div className="sk-tab-newbtn" onClick={() => setShowNewSession(true)} title="New session">
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
				first={sessions.map((s) => (
					<div
						key={s.id}
						style={{
							display: s.id === activeSessionId ? "flex" : "none",
							flexDirection: "column",
							flex: 1,
							minHeight: 0,
						}}
					>
						<HarnessColumn
							session={s}
							fontSize={fontSize}
							defaultShell={defaultShell}
							showPicker={showPicker === s.id}
							onPick={pickHarness}
							onAddHarness={addHarness}
							onSwitchHarness={switchHarnessInSession}
							onCloseHarness={closeHarness}
							onHarnessCmdChange={updateHarnessCmd}
						/>
					</div>
				))}
				second={sessions.map((s) => (
					<div
						key={s.id}
						className="sk-right"
						style={{
							display: s.id === activeSessionId ? "flex" : "none",
						}}
					>
						{s.cwd && <LiveStatus cwd={s.cwd} />}
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
				<span className="seg">{session.branch}</span>
				{session.cwd && (
					<span className="seg sk-statusbar-cwd" title={session.cwd}>
						{session.cwd}
					</span>
				)}
				<span className="spacer" />
				<span className="seg">utf-8 · LF</span>
			</div>

			{showNewSession && (
				<NewSessionDialog
					defaultCwd={defaultCwd}
					onCommit={createSession}
					onCancel={() => setShowNewSession(false)}
				/>
			)}
			{showPalette && <CommandPalette items={paletteItems} onClose={() => setShowPalette(false)} />}
			{showSettings && <SettingsModal {...settingsProps} />}
		</div>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
