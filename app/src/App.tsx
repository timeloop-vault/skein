// Skein interactive prototype — single React tree, ported from the
// design handoff in _design/skein/project/skein-proto.jsx.
//
// Mental model:
//   - Tabs along the top are sessions (workspace = repo + branch + task).
//   - Each session owns N harnesses (Claude Code, opencode, gh copilot,
//     or a built-in BYOH agent). All harnesses in a session share the
//     same worktree.
//   - The right pane belongs to the session, not the harness — so when
//     you switch agents inside the same workspace, the diff/files/plan
//     stay put.

import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { LiveTerminal } from "./LiveTerminal.tsx";
import {
	ActivityFeed,
	ByohPanel,
	ByohResolvedPanel,
	ClaudePanel,
	CopilotErroredPanel,
	CopilotPanel,
	DiffEditor,
	FileTree,
	FullPaneHead,
	HChip,
	HarnessPicker,
	HarnessTab,
	OpenCodePanel,
	PlanCard,
	SessionTab,
	StatusDot,
} from "./components.tsx";
import { HARNESS_KINDS, HARNESS_ORDER, INITIAL_SESSIONS, SESSION_DATA } from "./data.tsx";
import type {
	Density,
	Harness,
	HarnessKind,
	RightTab,
	Session,
	SessionData,
	Theme,
} from "./types.ts";

// ── Harness body routing ───────────────────────────────────────────

const HarnessBody = ({
	harness,
	resolved,
	onApprove,
	onRetry,
	onReauth,
}: {
	harness: Harness;
	resolved: boolean;
	onApprove: () => void;
	onRetry: () => void;
	onReauth: () => void;
}) => {
	// Live harnesses (spawned via "+ harness") run inside a real PTY.
	// Seeded demo harnesses (s1-s5) keep the frozen TUI mocks so the
	// design tour still tells its story end-to-end.
	if (harness.live && harness.cmd && harness.cwd !== undefined) {
		return <LiveTerminal cmd={harness.cmd} cwd={harness.cwd} mountKey={harness.id} />;
	}
	if (harness.kind === "byoh" && harness.status === "waiting" && !resolved) {
		return <ByohPanel harnessId={harness.id} onApprove={onApprove} />;
	}
	if (harness.kind === "byoh" && harness.status === "waiting" && resolved) {
		return <ByohResolvedPanel harnessId={harness.id} />;
	}
	if (harness.status === "error") {
		return <CopilotErroredPanel harnessId={harness.id} onRetry={onRetry} onReauth={onReauth} />;
	}
	switch (harness.kind) {
		case "claude":
			return <ClaudePanel harnessId={harness.id} />;
		case "opencode":
			return <OpenCodePanel harnessId={harness.id} />;
		case "copilot":
			return <CopilotPanel harnessId={harness.id} />;
		case "byoh":
			return <ByohPanel harnessId={harness.id} onApprove={onApprove} />;
	}
};

// ── Right-pane variants ────────────────────────────────────────────

const ContextStack = ({ data, showActivity }: { data: SessionData; showActivity: boolean }) => {
	const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
	const toggle = (k: string) => setCollapsed((p) => ({ ...p, [k]: !p[k] }));
	const filename = data.activeFile.path.split("/").pop() ?? data.activeFile.path;
	const planDone = data.plan.filter((t) => t.state === "done").length;
	return (
		<div className="sk-context-stack">
			<div className={`sk-context-card h-2 ${collapsed.diff ? "collapsed" : ""}`}>
				<div className="sk-context-head" onClick={() => toggle("diff")}>
					<span className="chev">▾</span>
					<span className="live" />
					<span className="label">Diff · {filename}</span>
					<span className="meta">
						+{data.activeFile.adds} −{data.activeFile.dels} · live
					</span>
				</div>
				<div className="sk-context-body">
					<FileTree tree={data.tree} />
					<DiffEditor activeFile={data.activeFile} diff={data.diff} />
				</div>
			</div>
			<div className={`sk-context-card h-1 ${collapsed.plan ? "collapsed" : ""}`}>
				<div className="sk-context-head" onClick={() => toggle("plan")}>
					<span className="chev">▾</span>
					<span className="label">Plan</span>
					<span className="meta">
						{planDone}/{data.plan.length} done
					</span>
				</div>
				<div className="sk-context-body" style={{ overflow: "auto" }}>
					<PlanCard plan={data.plan} />
				</div>
			</div>
			{showActivity && (
				<div className={`sk-context-card h-1 ${collapsed.activity ? "collapsed" : ""}`}>
					<div className="sk-context-head" onClick={() => toggle("activity")}>
						<span className="chev">▾</span>
						<span className="label">Activity · all harnesses</span>
						<span className="meta">{data.activity.length} events</span>
					</div>
					<div className="sk-context-body" style={{ overflow: "auto" }}>
						<ActivityFeed activity={data.activity} />
					</div>
				</div>
			)}
		</div>
	);
};

const FilesFullPane = ({ data }: { data: SessionData }) => (
	<div className="sk-fullpane">
		<FullPaneHead
			title={`Files · ${data.activeFile.path}`}
			meta={`+${data.activeFile.adds} −${data.activeFile.dels}`}
		/>
		<div className="sk-fullpane-body sk-fullpane-files">
			<FileTree tree={data.tree} />
			<DiffEditor activeFile={data.activeFile} diff={data.diff} />
		</div>
	</div>
);

const DiffFullPane = ({ data }: { data: SessionData }) => (
	<div className="sk-fullpane">
		<FullPaneHead
			title={`Diff · ${data.activeFile.path}`}
			meta={`+${data.activeFile.adds} −${data.activeFile.dels} · since session start`}
		/>
		<div className="sk-fullpane-body">
			<DiffEditor activeFile={data.activeFile} diff={data.diff} />
		</div>
	</div>
);

const PlanFullPane = ({ data }: { data: SessionData }) => {
	const planDone = data.plan.filter((t) => t.state === "done").length;
	return (
		<div className="sk-fullpane">
			<FullPaneHead title="Plan" meta={`${planDone}/${data.plan.length} done`} />
			<div className="sk-fullpane-body sk-fullpane-plan">
				<PlanCard plan={data.plan} />
				<div className="sk-fullpane-section">
					<div className="sec-label">Recent activity</div>
					<ActivityFeed activity={data.activity} />
				</div>
			</div>
		</div>
	);
};

// ── New session dialog ─────────────────────────────────────────────
// Phase 2: replaces the design's fictional REPO_PRESETS dropdown with
// a real folder picker. The picked path becomes the session's cwd, and
// every harness in the session spawns into it. Branch is still
// cosmetic — wired for real in Phase 4.

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

// ── Tour ───────────────────────────────────────────────────────────

interface TourActions {
	setShowNewSession: (v: boolean) => void;
	setActiveSessionId: (id: string) => void;
	setToastDismissed: (v: boolean) => void;
	switchHarnessInSession: (sessionId: string, harnessId: string) => void;
	approve: (harnessId: string) => void;
}

interface TourStep {
	target: string | null;
	place: "top" | "bottom" | "left" | "right" | "center";
	title: string;
	body: ReactNode;
	action?: (a: TourActions) => void;
	advance: "auto" | "manual";
	delay?: number;
}

const TOUR_STEPS: TourStep[] = [
	{
		target: ".sk-tabstrip",
		place: "bottom",
		title: "Sessions, not chats",
		body: (
			<>
				Each tab is a <em>workspace</em> — one repo, one task. The status dot tells you whether it's
				running, waiting, idle, or errored.
			</>
		),
		advance: "auto",
		delay: 3800,
	},
	{
		target: ".sk-tab-newbtn",
		place: "bottom",
		title: "Create a new session",
		body: <>Pick a repo, name the task, and Skein puts you on a fresh worktree branch.</>,
		action: (a) => a.setShowNewSession(true),
		advance: "auto",
		delay: 3000,
	},
	{
		target: ".sk-modal",
		place: "right",
		title: "Two real decisions",
		body: (
			<>
				Repo and task. Branch defaults to a new worktree (you can put a Claude harness on{" "}
				<code>feat/x</code> while a Copilot harness fixes <code>main</code>).
			</>
		),
		advance: "auto",
		delay: 4200,
	},
	{
		target: null,
		place: "center",
		title: "Skipping the form for now",
		body: <>We'll close this and look at a workspace that already has work in flight.</>,
		action: (a) => {
			a.setShowNewSession(false);
			a.setActiveSessionId("s1");
		},
		advance: "auto",
		delay: 2400,
	},
	{
		target: ".sk-harness-tabs",
		place: "bottom",
		title: "Harnesses live inside a session",
		body: (
			<>
				This session has Claude Code <em>and</em> opencode running on the same worktree. They see
				each other's edits.
			</>
		),
		advance: "auto",
		delay: 3800,
	},
	{
		target: ".sk-harness-col",
		place: "right",
		title: "Each harness is a real TUI",
		body: (
			<>
				We're emulating <code>claude</code>, <code>opencode</code>, <code>gh copilot</code>, and a
				built-in agent — same fingerprints you'd see in your terminal.
			</>
		),
		action: (a) => a.switchHarnessInSession("s1", "h1b"),
		advance: "auto",
		delay: 4400,
	},
	{
		target: ".sk-right",
		place: "left",
		title: "The worktree is shared",
		body: (
			<>
				Switch harnesses, the diff and plan stay put. They're a property of the <em>workspace</em>,
				not the agent. opencode just flagged a token mismatch in Claude's diff — that's the
				cross-harness conversation.
			</>
		),
		advance: "auto",
		delay: 4800,
	},
	{
		target: ".sk-toast, .sk-statusbar .urgent",
		place: "top",
		title: "Ambient signal",
		body: <>Another session needs you. The toast and status-bar segment both deep-link there.</>,
		action: (a) => {
			a.setActiveSessionId("s2");
			a.setToastDismissed(false);
		},
		advance: "auto",
		delay: 3600,
	},
	{
		target: ".sk-harness-col",
		place: "right",
		title: "Permission, inline",
		body: (
			<>
				The BYOH agent paused for <code>cargo test</code>. You approve in-place — no modal, no
				context switch.
			</>
		),
		advance: "auto",
		delay: 3400,
	},
	{
		target: ".sk-harness-col",
		place: "right",
		title: "Approving…",
		body: <>Watch the agent continue, the tests pass, and the status flip green.</>,
		action: (a) => a.approve("h2a"),
		advance: "auto",
		delay: 3600,
	},
	{
		target: ".sk-tabstrip",
		place: "bottom",
		title: "Errors are scoped",
		body: (
			<>
				Now jump to the spike — Copilot's token expired mid-stream. Worktree is safe; only the
				harness died.
			</>
		),
		action: (a) => a.setActiveSessionId("s5"),
		advance: "auto",
		delay: 3800,
	},
	{
		target: ".sk-harness-col",
		place: "right",
		title: "Recover in place",
		body: (
			<>
				Re-auth, retry, or hand the work to a different harness. Because session ≠ harness, none of
				this loses your context.
			</>
		),
		advance: "auto",
		delay: 3800,
	},
	{
		target: null,
		place: "center",
		title: "That's Skein.",
		body: (
			<>
				Sessions you can leave and come back to. Multiple agents on one worktree. Failures that
				don't take the room down with them. Hit Restart any time.
			</>
		),
		advance: "manual",
	},
];

interface CalloutStyle {
	left: string | number;
	top: string | number;
	width: number;
	transform?: string;
}

const TourOverlay = ({
	step,
	idx,
	total,
	onNext,
	onPrev,
	onSkip,
	onRestart,
}: {
	step: TourStep;
	idx: number;
	total: number;
	onNext: () => void;
	onPrev: () => void;
	onSkip: () => void;
	onRestart: () => void;
}) => {
	const [rect, setRect] = useState<DOMRect | null>(null);

	useEffect(() => {
		const measure = () => {
			if (!step.target) {
				setRect(null);
				return;
			}
			const selectors = step.target.split(",").map((s) => s.trim());
			let el: Element | null = null;
			for (const s of selectors) {
				el = document.querySelector(s);
				if (el) break;
			}
			if (el) {
				setRect(el.getBoundingClientRect());
			} else {
				setRect(null);
			}
		};
		measure();
		const t = window.setTimeout(measure, 60);
		window.addEventListener("resize", measure);
		return () => {
			window.clearTimeout(t);
			window.removeEventListener("resize", measure);
		};
	}, [step]);

	useEffect(() => {
		if (step.advance !== "auto") return undefined;
		const t = window.setTimeout(onNext, step.delay ?? 3500);
		return () => window.clearTimeout(t);
	}, [step, onNext]);

	const calloutStyle: CalloutStyle = (() => {
		const PAD = 14;
		const W = 360;
		if (!rect || step.place === "center") {
			return {
				left: "50%",
				top: "50%",
				transform: "translate(-50%, -50%)",
				width: W,
			};
		}
		const winW = window.innerWidth;
		if (step.place === "bottom") {
			return {
				left: Math.max(20, Math.min(winW - W - 20, rect.left + rect.width / 2 - W / 2)),
				top: rect.top + rect.height + PAD,
				width: W,
			};
		}
		if (step.place === "top") {
			return {
				left: Math.max(20, Math.min(winW - W - 20, rect.left + rect.width / 2 - W / 2)),
				top: rect.top - PAD,
				transform: "translateY(-100%)",
				width: W,
			};
		}
		if (step.place === "right") {
			return {
				left: rect.left + rect.width + PAD,
				top: Math.max(20, rect.top + 40),
				width: W,
			};
		}
		// left
		return {
			left: rect.left - PAD,
			transform: "translateX(-100%)",
			top: Math.max(20, rect.top + 40),
			width: W,
		};
	})();

	return (
		<div className="sk-tour-overlay">
			{rect ? (
				<div
					className="sk-tour-spotlight"
					style={{
						top: rect.top - 6,
						left: rect.left - 6,
						width: rect.width + 12,
						height: rect.height + 12,
					}}
				/>
			) : (
				<div className="sk-tour-scrim" />
			)}
			<div className="sk-tour-callout" style={calloutStyle}>
				<div className="head">
					<span className="step">
						{idx + 1}/{total}
					</span>
					<span className="dot-row">
						{TOUR_STEPS.map((_, i) => (
							<span key={i} className={`d ${i === idx ? "on" : i < idx ? "past" : ""}`} />
						))}
					</span>
				</div>
				<div className="title">{step.title}</div>
				<div className="body">{step.body}</div>
				<div className="foot">
					<button className="ghost" onClick={onSkip}>
						Skip tour
					</button>
					<span className="spacer" />
					{idx > 0 && (
						<button className="ghost" onClick={onPrev}>
							← Back
						</button>
					)}
					{step.advance === "manual" ? (
						<button className="primary" onClick={onRestart}>
							Restart
						</button>
					) : (
						<button className="primary" onClick={onNext}>
							{idx === total - 1 ? "Done" : "Next →"}
						</button>
					)}
				</div>
			</div>
		</div>
	);
};

// ── Titlebar ───────────────────────────────────────────────────────

const Titlebar = ({ onTour }: { onTour: () => void }) => (
	<div className="sk-titlebar" data-tauri-drag-region>
		<div className="sk-traffic">
			<span className="sk-traffic-light close" />
			<span className="sk-traffic-light min" />
			<span className="sk-traffic-light max" />
		</div>
		<span className="sk-app-name">
			<span className="dot">●</span> skein
		</span>
		<button className="sk-tour-launch" onClick={onTour} type="button">
			▶ Take the tour
		</button>
	</div>
);

// ── App ────────────────────────────────────────────────────────────

const RIGHT_TABS: { id: RightTab; label: string }[] = [
	{ id: "stack", label: "Live context" },
	{ id: "files", label: "Files" },
	{ id: "diff", label: "Diff" },
	{ id: "plan", label: "Plan" },
];

const newId = (prefix: string): string => prefix + Math.random().toString(36).slice(2, 7);

// Phase 1 mapping from harness kind → argv. Each binary must be on PATH
// for the spawn to succeed; if it isn't, the LiveTerminal renders the
// error inline and the user can pick another kind.
const cmdForKind = (kind: HarnessKind, fallbackShell: string[]): string[] => {
	switch (kind) {
		case "claude":
			return ["claude"];
		case "opencode":
			return ["opencode"];
		case "copilot":
			return ["gh", "copilot", "suggest"];
		case "byoh":
			// No real BYOH agent yet — drop into the user's shell so the
			// spike still proves the wiring end-to-end.
			return fallbackShell.length > 0 ? fallbackShell : ["pwsh.exe"];
	}
};

export default function App() {
	const [theme, setTheme] = useState<Theme>("dark");
	const [density, setDensity] = useState<Density>("regular");
	const [showActivityFeed, setShowActivityFeed] = useState(true);

	const [sessions, setSessions] = useState<Session[]>(INITIAL_SESSIONS);
	const [activeSessionId, setActiveSessionId] = useState<string>("s2");
	const [permissionResolved, setPermissionResolved] = useState<Record<string, boolean>>({});
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [toastDismissed, setToastDismissed] = useState(false);
	const [showNewSession, setShowNewSession] = useState(false);
	const [rightTab, setRightTab] = useState<RightTab>("stack");
	const [tourIdx, setTourIdx] = useState<number | null>(null);

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
	// otherwise INITIAL_SESSIONS would clobber the DB before we read it.
	const [loaded, setLoaded] = useState(false);
	useEffect(() => {
		invoke<Session[]>("db_load_sessions")
			.then((rows) => {
				if (rows.length > 0) {
					setSessions(rows);
					const first = rows[0];
					if (first) setActiveSessionId(first.id);
				}
				// If empty, leave INITIAL_SESSIONS in state — the save
				// effect below will persist them on its first run, so
				// next boot picks them up from the DB.
				setLoaded(true);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] db_load_sessions failed:", msg);
				// Still flip `loaded` so the UI isn't stuck — we just
				// run without persistence for this session.
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
	const data: SessionData | undefined = session ? SESSION_DATA[session.id] : undefined;

	const switchSession = (id: string) => {
		setActiveSessionId(id);
		setToastDismissed(true);
	};

	const closeSession = (id: string) => {
		// Confirm before delete — sessions can hold a lot of state and
		// the prototype has no undo. window.confirm is fine for v0.
		if (!window.confirm("Close this session? Any running harnesses will be killed.")) {
			return;
		}
		setSessions((prev) => {
			const remaining = prev.filter((s) => s.id !== id);
			// If we just closed the active session, jump to the first
			// remaining one (or no-op if the list is now empty — the
			// empty state takes over on the next render).
			if (id === activeSessionId) {
				const first = remaining[0];
				if (first) setActiveSessionId(first.id);
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

	const addHarness = (sessionId: string) => setShowPicker(sessionId);

	const pickHarness = (kind: HarnessKind) => {
		const targetSessionId = showPicker;
		if (!targetSessionId) return;
		const id = newId("h");
		// Phase 1: argv per kind. Phase 2: spawn into the session's cwd
		// so two harnesses in the same session edit the same files.
		// Seeded sessions don't have a real cwd — fall back to defaultCwd.
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

	const approve = (harnessId: string) => {
		setPermissionResolved((prev) => ({ ...prev, [harnessId]: true }));
		setSessions((prev) =>
			prev.map((s) => {
				if (!s.harnesses.find((h) => h.id === harnessId)) return s;
				return {
					...s,
					status: "running",
					badge: 0,
					harnesses: s.harnesses.map((h) => (h.id === harnessId ? { ...h, status: "running" } : h)),
				};
			}),
		);
	};

	const recoverError = (harnessId: string) => {
		setSessions((prev) =>
			prev.map((s) => {
				if (!s.harnesses.find((h) => h.id === harnessId)) return s;
				return {
					...s,
					status: "running",
					badge: 0,
					harnesses: s.harnesses.map((h) => (h.id === harnessId ? { ...h, status: "running" } : h)),
				};
			}),
		);
	};

	const tourActions: TourActions = {
		setShowNewSession,
		setActiveSessionId,
		setToastDismissed,
		switchHarnessInSession,
		approve,
	};

	const startTour = () => {
		setSessions(INITIAL_SESSIONS);
		setActiveSessionId("s2");
		setPermissionResolved({});
		setToastDismissed(false);
		setShowNewSession(false);
		setRightTab("stack");
		setTourIdx(0);
	};

	const nextStep = () => {
		setTourIdx((i) => {
			if (i === null) return null;
			if (i >= TOUR_STEPS.length - 1) return null;
			return i + 1;
		});
	};
	const prevStep = () => setTourIdx((i) => (i === null || i <= 0 ? i : i - 1));
	const skipTour = () => {
		setTourIdx(null);
		setShowNewSession(false);
	};

	// biome-ignore lint/correctness/useExhaustiveDependencies: tourActions is reconstructed every render but its setters are stable; we deliberately fire only when tourIdx changes
	useEffect(() => {
		if (tourIdx === null) return;
		const step = TOUR_STEPS[tourIdx];
		step?.action?.(tourActions);
	}, [tourIdx]);

	const urgent = sessions.find((s) => s.id !== activeSessionId && s.status === "waiting");

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

	// Empty state — no sessions at all.
	if (sessions.length === 0) {
		return (
			<div className={`sk-app sk-${theme} density-${density}`}>
				<Titlebar onTour={startTour} />
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
	// `data` is only populated for the seeded demo sessions (s1-s5).
	// New sessions have a real cwd but no SESSION_DATA fixture yet —
	// the right pane just stays empty until Phase 5 wires real diffs.

	const tourStep = tourIdx !== null ? TOUR_STEPS[tourIdx] : undefined;

	return (
		<div className={`sk-app sk-${theme} density-${density}`}>
			<Titlebar onTour={startTour} />

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

			<div className="sk-workspace">
				<div className="sk-harness-col">
					<div className="sk-harness-tabs">
						{session.harnesses.map((h) => (
							<HarnessTab
								key={h.id}
								h={h}
								active={h.id === session.activeHarnessId}
								onClick={() => switchHarnessInSession(session.id, h.id)}
								onClose={() => closeHarness(session.id, h.id)}
							/>
						))}
						<div className="sk-harness-add" onClick={() => addHarness(session.id)}>
							+ harness
						</div>
						<div className="sk-harness-meta">
							<span>
								{session.repo} · {session.branch}
							</span>
						</div>
					</div>

					{showPicker === session.id ? (
						<HarnessPicker onPick={pickHarness} />
					) : (
						// Mount every harness in the active session at once; hide
						// inactive ones via display:none so xterm scrollback,
						// cursor position, and PTY state survive tab switches.
						// Mock (non-live) harnesses are re-rendered cheaply but
						// we use the same pattern for consistency.
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
									resolved={permissionResolved[h.id] ?? false}
									onApprove={() => approve(h.id)}
									onRetry={() => recoverError(h.id)}
									onReauth={() => recoverError(h.id)}
								/>
							</div>
						))
					)}
				</div>

				<div className="sk-right">
					<div className="sk-right-tabs">
						{RIGHT_TABS.map((rt) => (
							<div
								key={rt.id}
								className={`sk-right-tab ${rightTab === rt.id ? "active" : ""}`}
								onClick={() => setRightTab(rt.id)}
							>
								{rt.label}
							</div>
						))}
						<div className="sk-right-meta">
							<span className="live-dot" />
							<span>auto-follow</span>
						</div>
					</div>
					{data && rightTab === "stack" && (
						<ContextStack data={data} showActivity={showActivityFeed} />
					)}
					{data && rightTab === "files" && <FilesFullPane data={data} />}
					{data && rightTab === "diff" && <DiffFullPane data={data} />}
					{data && rightTab === "plan" && <PlanFullPane data={data} />}
					{!data && (
						<div
							style={{
								flex: 1,
								display: "flex",
								alignItems: "center",
								justifyContent: "center",
								color: "var(--fg-3)",
								fontFamily: "var(--sk-mono)",
								fontSize: 11,
								padding: 24,
								textAlign: "center",
							}}
						>
							{session.cwd ? (
								<>
									Live worktree: <span style={{ color: "var(--fg-1)" }}>{session.cwd}</span>
									<br />
									(Phase 5 will surface the diff here.)
								</>
							) : (
								"No data for this session yet."
							)}
						</div>
					)}
				</div>
			</div>

			<div className="sk-statusbar" style={{ position: "relative" }}>
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
				{urgent && (
					<span className="seg urgent" onClick={() => switchSession(urgent.id)}>
						<span className="dot-tiny st-waiting" />
						{urgent.name.split(" · ").pop()} needs you →
					</span>
				)}
				<span className="seg">utf-8 · LF</span>

				{urgent && !toastDismissed && (
					<UrgentToast
						urgent={urgent}
						onClick={() => switchSession(urgent.id)}
						onDismiss={() => setToastDismissed(true)}
					/>
				)}
			</div>

			{/* Lightweight in-window settings strip (replaces the design's Tweaks panel) */}
			<SettingsStrip
				theme={theme}
				density={density}
				showActivityFeed={showActivityFeed}
				onTheme={setTheme}
				onDensity={setDensity}
				onShowActivityFeed={setShowActivityFeed}
				onResetEmpty={() => setSessions([])}
				onRestoreSamples={() => {
					setSessions(INITIAL_SESSIONS);
					setActiveSessionId("s2");
					setPermissionResolved({});
					setToastDismissed(false);
				}}
			/>

			{showNewSession && (
				<NewSessionDialog
					defaultCwd={defaultCwd}
					onCommit={createSession}
					onCancel={() => setShowNewSession(false)}
				/>
			)}

			{tourIdx !== null && tourStep && (
				<TourOverlay
					step={tourStep}
					idx={tourIdx}
					total={TOUR_STEPS.length}
					onNext={nextStep}
					onPrev={prevStep}
					onSkip={skipTour}
					onRestart={startTour}
				/>
			)}
		</div>
	);
}

// ── Small in-window settings strip ─────────────────────────────────
// The design uses an external Tweaks panel; the prototype ships its
// settings inline so a single bundle stays self-contained.

const SettingsStrip = ({
	theme,
	density,
	showActivityFeed,
	onTheme,
	onDensity,
	onShowActivityFeed,
	onResetEmpty,
	onRestoreSamples,
}: {
	theme: Theme;
	density: Density;
	showActivityFeed: boolean;
	onTheme: (v: Theme) => void;
	onDensity: (v: Density) => void;
	onShowActivityFeed: (v: boolean) => void;
	onResetEmpty: () => void;
	onRestoreSamples: () => void;
}) => (
	<div
		style={{
			position: "fixed",
			right: 12,
			top: 38,
			display: "flex",
			gap: 6,
			alignItems: "center",
			background: "var(--bg-2)",
			border: "1px solid var(--line)",
			borderRadius: 6,
			padding: "4px 8px",
			fontFamily: "var(--sk-mono)",
			fontSize: 10,
			color: "var(--fg-2)",
			zIndex: 50,
		}}
	>
		<button
			className="sk-btn ghost"
			onClick={() => onTheme(theme === "dark" ? "light" : "dark")}
			title="Toggle theme"
		>
			{theme}
		</button>
		<select
			className="sk-select"
			style={{ padding: "3px 6px", fontSize: 10 }}
			value={density}
			onChange={(e) => onDensity(e.target.value as Density)}
		>
			<option value="compact">compact</option>
			<option value="regular">regular</option>
			<option value="comfy">comfy</option>
		</select>
		<label
			style={{ display: "flex", gap: 4, alignItems: "center", cursor: "pointer" }}
			title="Cross-harness activity feed"
		>
			<input
				type="checkbox"
				checked={showActivityFeed}
				onChange={(e) => onShowActivityFeed(e.target.checked)}
			/>
			activity
		</label>
		<button className="sk-btn ghost" onClick={onResetEmpty} title="Reset to empty state">
			empty
		</button>
		<button className="sk-btn ghost" onClick={onRestoreSamples} title="Restore demo data">
			restore
		</button>
	</div>
);

// ── Urgent toast ───────────────────────────────────────────────────

const UrgentToast = ({
	urgent,
	onClick,
	onDismiss,
}: {
	urgent: Session;
	onClick: () => void;
	onDismiss: () => void;
}) => {
	const firstHarness = urgent.harnesses[0];
	if (!firstHarness) return null;
	const tail = urgent.name.split(" · ").pop() ?? urgent.name;
	return (
		<div className="sk-toast" onClick={onClick}>
			<HChip kind={firstHarness.kind} size={14} />
			<div>
				<div style={{ color: "var(--fg-0)" }}>{tail} needs permission</div>
				<div
					style={{
						color: "var(--fg-2)",
						fontFamily: "var(--sk-mono)",
						fontSize: 10,
						marginTop: 2,
					}}
				>
					cargo test fs::watcher
				</div>
			</div>
			<span
				style={{ color: "var(--fg-3)", marginLeft: 8 }}
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

// Make the harness column status bar surface a dot for at-a-glance scan.
// (Re-exported for completeness; not used elsewhere outside this file.)
export { StatusDot };
