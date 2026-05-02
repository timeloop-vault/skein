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
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { LiveStatus } from "./LiveStatus.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import { Splitter } from "./Splitter.tsx";
import { HChip, HarnessPicker, HarnessTab, SessionTab, StatusDot } from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER } from "./data.tsx";
import { usePersistedState } from "./prefs.ts";
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
				<span className="kbd">⌘ N</span>
				<span>New session</span>
			</div>
			<div className="row">
				<span className="kbd">⌘ ⇧ H</span>
				<span>Add harness to current session</span>
			</div>
			<div className="row">
				<span className="kbd">⌘ K</span>
				<span>Switch session / harness</span>
			</div>
		</div>
	</div>
);

// ── Titlebar ───────────────────────────────────────────────────────

interface TitlebarProps {
	theme: Theme;
	density: Density;
	fontSize: number;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onFontSize: (v: number) => void;
}

const Titlebar = ({ theme, density, fontSize, onTheme, onDensity, onFontSize }: TitlebarProps) => (
	<div className="sk-titlebar" data-tauri-drag-region>
		<span className="sk-app-name">
			<span className="dot">●</span> skein
		</span>
		{/* The settings group opts out of the drag region so its buttons
		    receive clicks instead of starting a window drag. */}
		<div className="sk-settings" data-tauri-drag-region="false">
			<button
				className="sk-btn ghost"
				onClick={() => onTheme(theme === "dark" ? "light" : "dark")}
				title="Toggle theme"
			>
				{theme}
			</button>
			<select
				className="sk-select sk-settings-select"
				value={density}
				onChange={(e) => onDensity(e.target.value as Density)}
				title="UI density"
			>
				<option value="compact">compact</option>
				<option value="regular">regular</option>
				<option value="comfy">comfy</option>
			</select>
			<div className="sk-font-group" title="Terminal font size">
				<button
					className="sk-btn ghost"
					onClick={() => onFontSize(Math.max(FONT_MIN, fontSize - 1))}
					disabled={fontSize <= FONT_MIN}
				>
					−
				</button>
				<span className="sk-font-size">{fontSize}</span>
				<button
					className="sk-btn ghost"
					onClick={() => onFontSize(Math.min(FONT_MAX, fontSize + 1))}
					disabled={fontSize >= FONT_MAX}
				>
					+
				</button>
			</div>
		</div>
	</div>
);

// ── App ────────────────────────────────────────────────────────────

const newId = (prefix: string): string => prefix + Math.random().toString(36).slice(2, 7);

// Mapping from harness kind → argv. Each binary must be on PATH for the
// spawn to succeed; if it isn't, the LiveTerminal renders the error
// inline and the user can pick another kind. The `byoh` kind is our
// "Shell" option — it drops into the user's default shell so they can
// run whatever they want.
const cmdForKind = (kind: HarnessKind, fallbackShell: string[]): string[] => {
	switch (kind) {
		case "claude":
			return ["claude"];
		case "opencode":
			return ["opencode"];
		case "copilot":
			return ["gh", "copilot", "suggest"];
		case "byoh":
			return fallbackShell.length > 0 ? fallbackShell : ["pwsh.exe"];
	}
};

// Phase 5a: rewrite a stored cmd into its "resume the previous
// conversation" form, applied once at boot so a fresh PTY spawn
// transparently re-attaches. Only matches the canonical kind-default
// argv — anything customized (shell-swapped via phase 4's onCmdChange,
// user-edited extra args) passes through unchanged.
//
// gh copilot has no resume mode; shells start fresh; opencode resumes
// the most recent conversation in the cwd; Claude shows a picker.
const resumeCmd = (kind: HarnessKind, cmd: string[]): string[] => {
	if (kind === "claude" && cmd.length === 1 && cmd[0] === "claude") {
		return ["claude", "--resume"];
	}
	if (kind === "opencode" && cmd.length === 1 && cmd[0] === "opencode") {
		return ["opencode", "--continue"];
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

	// Ctrl+= / Ctrl++ to bump font size, Ctrl+- to shrink. Window-level so
	// it works no matter which pane has focus, including the terminal.
	// preventDefault stops the WebView's built-in zoom.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (!e.ctrlKey || e.altKey || e.metaKey) return;
			if (e.key === "+" || e.key === "=") {
				e.preventDefault();
				setFontSize((s) => Math.min(FONT_MAX, s + 1));
			} else if (e.key === "-") {
				e.preventDefault();
				setFontSize((s) => Math.max(FONT_MIN, s - 1));
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [setFontSize]);

	const [sessions, setSessions] = useState<Session[]>([]);
	const [activeSessionId, setActiveSessionId] = useState<string>("");
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [showNewSession, setShowNewSession] = useState(false);

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
		invoke<Session[]>("db_load_sessions")
			.then((rows) => {
				if (rows.length > 0) {
					// Phase 5a: rewrite each harness's cmd to its resume form
					// before mounting, so the PTY spawn re-attaches to the
					// prior conversation instead of starting fresh.
					const withResume = rows.map((s) => ({
						...s,
						harnesses: s.harnesses.map((h) =>
							h.cmd ? { ...h, cmd: resumeCmd(h.kind, h.cmd) } : h,
						),
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

	const pickHarness = (kind: HarnessKind) => {
		const targetSessionId = showPicker;
		if (!targetSessionId) return;
		const id = newId("h");
		const cmd = cmdForKind(kind, defaultShell);
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
					cwd: s.cwd ?? defaultCwd,
				};
				return { ...s, harnesses: [...s.harnesses, newH], activeHarnessId: id };
			}),
		);
		setShowPicker(null);
	};

	const createSession = ({ cwd, task, harness, branch }: CreateSessionArgs) => {
		const sid = newId("s");
		const hid = newId("h");
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
					cmd: cmdForKind(harness, defaultShell),
					cwd,
				},
			],
			activeHarnessId: hid,
		};
		setSessions((prev) => [...prev, newSession]);
		setActiveSessionId(sid);
		setShowNewSession(false);
	};

	const titlebarProps: TitlebarProps = {
		theme,
		density,
		fontSize,
		onTheme: setTheme,
		onDensity: setDensity,
		onFontSize: setFontSize,
	};

	// Empty state — no sessions at all.
	if (sessions.length === 0) {
		return (
			<div className={`sk-app sk-${theme} density-${density}`}>
				<Titlebar {...titlebarProps} />
				<EmptyState onNew={() => setShowNewSession(true)} />
				{showNewSession && (
					<NewSessionDialog
						defaultCwd={defaultCwd}
						onCommit={createSession}
						onCancel={() => setShowNewSession(false)}
					/>
				)}
			</div>
		);
	}

	if (!session || !activeHarness) {
		// Shouldn't happen in practice: sessions is non-empty above.
		return null;
	}

	return (
		<div className={`sk-app sk-${theme} density-${density}`}>
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
				<span className="seg">{activeHarness.model}</span>
				<span className="seg">{activeHarness.tokens} tok</span>
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
		</div>
	);
}

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
