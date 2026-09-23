import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import type { CreateRoomArgs, FolderInfoDto, RepoStatus } from "./NewRoomDialogTypes.ts";
import { kindHasAgents } from "./agents.ts";
import {
	applyBranchTemplate,
	branchFieldProblem,
	taskSlug,
	templateFromBranch,
	worktreeLeaf,
} from "./branchName.ts";
import { useAgentListing } from "./components.tsx";
import {
	type DefaultAgents,
	type FolderDefaults,
	type NewRoomMemory,
	type RecentFolder,
	branchTemplateFor,
	startingAgent,
} from "./prefs.ts";
import type { HarnessKind } from "./types.ts";

// Split out of `NewRoomDialog.tsx` (#19): the folder-inspection /
// branch-validation state, its effects, and the handlers that act on it.
// `NewRoomDialog` itself stays the JSX; this hook is everything that state
// machine needs to answer "what would submitting right now create".

export const useNewRoomForm = ({
	defaultCwd,
	initialCwd,
	initialDefaults,
	defaultAgents,
	memory,
	appBranchTemplate,
	onRemember,
	onCommit,
}: {
	defaultCwd: string;
	/** Folder to open with — the active room's, or the last one used (#226). */
	initialCwd: string;
	/** Per-folder defaults for `initialCwd`, when we have seen it before. */
	initialDefaults: FolderDefaults | undefined;
	/** Settings' per-kind default agents (#248). A folder's own memory
	 *  still wins — see `startingAgent`. */
	defaultAgents: DefaultAgents;
	/** New Room memory (#227): looked up per-folder as `cwd` resolves, so
	 *  the proposed branch's template can follow the folder rather than
	 *  only the one it opened on. */
	memory: NewRoomMemory;
	/** Settings' app-wide branch template (#227). A folder's own
	 *  remembered template still wins — see `branchTemplateFor`. */
	appBranchTemplate: string;
	/** Called with the folder and its defaults after a room is created. */
	onRemember: (folder: string, defaults: Omit<FolderDefaults, "lastUsed">) => void;
	onCommit: (args: CreateRoomArgs) => void;
}) => {
	const [cwd, setCwd] = useState<string>(initialCwd);
	const [task, setTask] = useState("");
	const [harness, setHarness] = useState<HarnessKind>(initialDefaults?.harness ?? "claude");
	// `undefined` = the tool's own default, which is a selectable row in
	// the field rather than the absence of a selection (#247).
	const [agent, setAgent] = useState<string | undefined>(() =>
		startingAgent(initialDefaults, initialDefaults?.harness ?? "claude", defaultAgents),
	);
	const [branchMode, setBranchMode] = useState<"worktree" | "current">(
		initialDefaults?.branchMode ?? "worktree",
	);
	const [baseBranch, setBaseBranch] = useState<string>(initialDefaults?.baseBranch ?? "");
	const [repoStatus, setRepoStatus] = useState<RepoStatus>({ kind: "empty" });
	// The folder the validation effect last resolved — a repo root or a
	// plain directory that exists. What #247's agent probe is keyed on;
	// see `agentListing`.
	const [settledCwd, setSettledCwd] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	// Sticky, and deliberately not part of `repoStatus`: resolving a
	// worktree rewrites `cwd`, which re-runs the validation effect, and
	// that second pass sees an ordinary repo. Kept out here the note
	// survives revalidation instead of flashing once and vanishing.
	// Cleared whenever the user picks or types a folder themselves.
	const [resolvedFromWorktree, setResolvedFromWorktree] = useState(false);
	// Recent-folders dropdown (#233).
	const [showRecent, setShowRecent] = useState(false);
	const recentRef = useRef<HTMLDivElement | null>(null);

	// Close the dropdown on any pointer press outside it. Bound on
	// mousedown rather than click so it closes before the press lands on
	// whatever is underneath, and only while open so the listener is not
	// carried by every dialog that never opens it.
	useEffect(() => {
		if (!showRecent) return undefined;
		const onDown = (e: MouseEvent) => {
			if (!recentRef.current?.contains(e.target as Node)) setShowRecent(false);
		};
		document.addEventListener("mousedown", onDown);
		return () => document.removeEventListener("mousedown", onDown);
	}, [showRecent]);

	const pickRecent = (r: RecentFolder) => {
		// Goes through the same state the text field and Browse… write, so
		// validation, worktree resolution and the `missing` check all run
		// exactly as they would for a hand-typed path.
		setResolvedFromWorktree(false);
		setCwd(r.folder);
		// Picking from the list is an explicit "take me to that folder", so
		// its remembered defaults come along — otherwise per-folder memory
		// would apply on open but not on switch, which is the whole point
		// of the dropdown. Typing or browsing deliberately does not do
		// this: silently changing the harness under someone mid-edit is a
		// surprise, and neither gesture names a folder we already know.
		setHarness(r.defaults.harness);
		setAgent(startingAgent(r.defaults, r.defaults.harness, defaultAgents));
		setBranchMode(r.defaults.branchMode);
		// The validation effect drops this again if the branch is gone.
		setBaseBranch(r.defaults.baseBranch);
		// #227 review: re-attach the branch field so the picked folder's
		// own remembered template applies, rather than carrying over
		// whatever the previous folder's field held (typed or proposed).
		setBranchAttached(true);
		setShowRecent(false);
	};

	// Validate the picked folder + load branches. Debounced so typing in
	// the path field doesn't fire one round-trip per keystroke.
	//
	// #181: `cancelled` lives in the effect body, not inside the timeout
	// callback, so the effect's own cleanup can actually set it. In the
	// previous shape the cleanup was returned from the timeout callback,
	// where nothing ever called it — harmless while the field started
	// blank (the effect never fired on open), but a prefilled field fires
	// it immediately, so a slow response for the remembered path could
	// land *after* the user typed a different one and overwrite it.
	useEffect(() => {
		setError(null);
		if (!cwd) {
			setRepoStatus({ kind: "empty" });
			setSettledCwd("");
			return undefined;
		}
		setRepoStatus({ kind: "checking" });
		let cancelled = false;
		const handle = window.setTimeout(() => {
			void (async () => {
				try {
					const info = await invoke<FolderInfoDto>("git_inspect_folder", { path: cwd });
					if (cancelled) return;
					if (!info.exists) {
						setRepoStatus({ kind: "missing" });
						// A folder that is not there has no project agents and
						// cannot be a room. Clearing rather than leaving the last
						// good one keeps the Agent field from describing a folder
						// the user has navigated away from.
						setSettledCwd("");
						return;
					}
					if (!info.isRepo) {
						setRepoStatus({ kind: "not-a-repo" });
						setSettledCwd(cwd);
						return;
					}
					// A worktree resolves to the repo it was added from —
					// otherwise we would stack a worktree on a worktree, which
					// is one misclick away given the `-wt` sibling dir (#226).
					if (info.resolvedFromWorktree) {
						setResolvedFromWorktree(true);
						setCwd(info.root);
					}
					setRepoStatus({ kind: "valid", branches: info.branches, head: info.head });
					// `info.root`, not `cwd`: a worktree resolves to the repo it
					// was added from, and the rewrite of `cwd` above lands a
					// render later.
					setSettledCwd(info.root);
					// Default base branch to HEAD on first valid load. A
					// remembered branch survives only if the repo still has
					// it — otherwise the <select> would sit on a value with
					// no matching <option>, render blank, and submit it.
					setBaseBranch((prev) => {
						const remembered = info.branches.some((b) => b.name === prev);
						if (prev && remembered) return prev;
						return info.head || info.branches[0]?.name || "";
					});
				} catch (err: unknown) {
					if (cancelled) return;
					const msg = err instanceof Error ? err.message : String(err);
					setError(`git: ${msg}`);
					setRepoStatus({ kind: "not-a-repo" });
				}
			})();
		}, 200);
		return () => {
			cancelled = true;
			window.clearTimeout(handle);
		};
	}, [cwd]);

	const slug = taskSlug(task);
	// The resolved folder, once validation has settled — falls back to
	// the raw `cwd` before the first settle so the template has *some*
	// folder to key on rather than always reading the app-wide default
	// on open.
	const branchTemplateFolder = settledCwd || cwd;
	const proposedBranch = applyBranchTemplate(
		branchTemplateFor(memory, branchTemplateFolder, appBranchTemplate),
		slug,
	);

	// #227: the branch field follows `proposedBranch` until the user
	// types in it, then detaches — typing is a deliberate override.
	// Blurring an empty field re-attaches (`branchFieldAttachedAfterBlur`).
	const [branch, setBranch] = useState(proposedBranch);
	const [branchAttached, setBranchAttached] = useState(true);
	useEffect(() => {
		if (branchAttached) setBranch(proposedBranch);
	}, [proposedBranch, branchAttached]);

	// #247: the agents the chosen kind will accept in this folder.
	//
	// Keyed on the *settled* folder, not on `cwd`: the probe runs the
	// harness CLI, and `cwd` changes on every keystroke in the path
	// field. `settledCwd` only moves once the folder has resolved, which
	// the git validation below already debounces — so a typed path costs
	// one probe, not one per character.
	//
	// That folder is the picked one, which for a worktree room is the
	// repo root rather than the worktree that does not exist yet. The
	// worktree is branched off this repo, so it carries the same
	// `.claude/agents`; every other source (user dir, plugins) is
	// cwd-independent anyway.
	const agentListing = useAgentListing(kindHasAgents(harness) ? harness : null, settledCwd);

	// A remembered agent the CLI no longer offers. Only ever set when
	// the list is authoritative — a degraded list cannot prove absence,
	// and saying "not found" on the strength of a CLI that would not run
	// is how a user ends up retyping a name that was fine.
	const agentMissing =
		agent !== undefined &&
		agentListing !== null &&
		agentListing.degraded === null &&
		!agentListing.agents.some((a) => a.name === agent);

	const isRepo = repoStatus.kind === "valid";
	// `missing` is deliberately excluded: a folder that isn't there is the
	// one unresolved state that must block submission (#226).
	const folderResolved = repoStatus.kind === "valid" || repoStatus.kind === "not-a-repo";

	// #227: the worktree folder a submit would create at the current
	// branch name, and whether one is already sitting there — debounced
	// and cancelled like the folder-validation effect above (#181: the
	// cancellation there was dead code until a prefilled field made a
	// stale response actually reachable; the same shape applies here).
	const [worktreePath, setWorktreePath] = useState("");
	const [worktreeFolderExists, setWorktreeFolderExists] = useState(false);
	// #227 review: `worktreeFolderExists` only reflects the *last landed*
	// probe, which `canCreate` read synchronously — a branch typed after
	// the last probe answered could submit before this one comes back.
	// Tracked separately from `busy`/`repoStatus.kind === "checking"`
	// because it is its own async gate with its own debounce.
	const [worktreeCheckPending, setWorktreeCheckPending] = useState(false);
	useEffect(() => {
		if (!isRepo || branchMode !== "worktree" || !branch.trim()) {
			setWorktreePath("");
			setWorktreeFolderExists(false);
			setWorktreeCheckPending(false);
			return undefined;
		}
		setWorktreeCheckPending(true);
		let cancelled = false;
		const handle = window.setTimeout(() => {
			void (async () => {
				try {
					const path = await invoke<string>("git_propose_worktree_path", {
						repoPath: cwd,
						taskSlug: worktreeLeaf(branch),
					});
					if (cancelled) return;
					setWorktreePath(path);
					const info = await invoke<FolderInfoDto>("git_inspect_folder", { path });
					if (cancelled) return;
					setWorktreeFolderExists(info.exists);
				} catch {
					if (cancelled) return;
					setWorktreePath("");
					setWorktreeFolderExists(false);
				} finally {
					if (!cancelled) setWorktreeCheckPending(false);
				}
			})();
		}, 200);
		return () => {
			cancelled = true;
			window.clearTimeout(handle);
		};
	}, [isRepo, branchMode, branch, cwd]);

	const branchProblem =
		isRepo && branchMode === "worktree" && repoStatus.kind === "valid"
			? branchFieldProblem(
					branch,
					repoStatus.branches.map((b) => b.name),
					worktreeFolderExists,
				)
			: null;
	// The last two path segments, e.g. `skein-wt/fix-x` — the card is too
	// narrow for the full absolute path, which still shows in the field's
	// `title` on hover.
	const worktreeShortPath = worktreePath.split(/[/\\]/).filter(Boolean).slice(-2).join("/");

	// Submit is fine for both git-backed and plain folders. The branch /
	// worktree picker only gates submission when the folder *is* a repo.
	const canCreate =
		task.trim().length > 0 &&
		!busy &&
		folderResolved &&
		// #247: a name the CLI just told us it does not have would spawn
		// a harness that refuses to start (Claude) or quietly runs as
		// something else (opencode). Blocking here is cheap — the field
		// is right there — and it is the only place that choice can be
		// corrected without creating a room first.
		!agentMissing &&
		(!isRepo ||
			branchMode === "current" ||
			(baseBranch.length > 0 && branchProblem === null && !worktreeCheckPending));

	const browse = async () => {
		const start = cwd || defaultCwd;
		const picked = await openDialog({
			directory: true,
			multiple: false,
			title: "Pick a folder for this room",
			...(start ? { defaultPath: start } : {}),
		});
		if (typeof picked === "string") {
			setResolvedFromWorktree(false);
			setCwd(picked);
			// #227 review: same as `pickRecent` — a folder picked via the OS
			// dialog is as explicit a folder change as one from the recent
			// list, so the branch field goes back to following that
			// folder's own template.
			setBranchAttached(true);
		}
	};

	const submit = async () => {
		if (!canCreate) return;
		setBusy(true);
		setError(null);
		// Called only on a path that genuinely created the room — a create
		// that throws must not teach the dialog anything.
		const remember = (base: string, branchTemplate?: string) =>
			onRemember(cwd, {
				baseBranch: base,
				harness,
				branchMode,
				...(agent ? { agent } : {}),
				...(branchTemplate ? { branchTemplate } : {}),
			});
		try {
			if (!isRepo) {
				// Non-git folder — no worktree, no branch. cwd is the
				// picked folder verbatim, and it is remembered like any
				// other folder (#231), with an empty base branch: there is
				// no branch to carry forward, but the folder itself and the
				// starting harness are worth exactly as much here.
				remember("");
				onCommit({
					cwd,
					task: task.trim(),
					harness,
					...(agent ? { agent } : {}),
				});
				return;
			}
			if (branchMode === "worktree") {
				const newWorktreePath = await invoke<string>("git_propose_worktree_path", {
					repoPath: cwd,
					taskSlug: worktreeLeaf(branch),
				});
				const wt = await invoke<{ name: string; path: string }>("git_add_worktree", {
					repoPath: cwd,
					branch,
					baseBranch,
					worktreePath: newWorktreePath,
				});
				// #227: only a successful create teaches the folder its
				// branch template — a failed one must not poison the next
				// open with a prefix that never actually landed.
				remember(baseBranch, templateFromBranch(branch));
				onCommit({
					cwd: wt.path,
					task: task.trim(),
					harness,
					...(agent ? { agent } : {}),
					branch,
					repoRoot: settledCwd,
				});
			} else {
				remember(baseBranch);
				onCommit({
					cwd,
					task: task.trim(),
					harness,
					...(agent ? { agent } : {}),
					branch: repoStatus.kind === "valid" ? (repoStatus.head ?? "HEAD") : "HEAD",
					repoRoot: settledCwd,
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
				// The worktree's *name* is deliberately not repeated here: the
				// blurb slot is ~82 characters wide and a real branch name eats
				// most of it, and the user just browsed there anyway. The only
				// thing they need told is that the field moved.
				return (
					<span style={{ color: "var(--ok)" }}>
						✓ git repo{repoStatus.head ? ` (HEAD: ${repoStatus.head})` : ""}
						{resolvedFromWorktree ? " · resolved from a worktree" : ""}
					</span>
				);
			case "not-a-repo":
				return (
					<span style={{ color: "var(--fg-3)" }}>
						not a git repo — harnesses run in this folder as-is.
					</span>
				);
			case "missing":
				return <span style={{ color: "var(--err)" }}>folder not found — pick another.</span>;
		}
	})();

	return {
		cwd,
		setCwd,
		task,
		setTask,
		harness,
		setHarness,
		agent,
		setAgent,
		branchMode,
		setBranchMode,
		baseBranch,
		setBaseBranch,
		repoStatus,
		setResolvedFromWorktree,
		showRecent,
		setShowRecent,
		recentRef,
		pickRecent,
		branch,
		setBranch,
		setBranchAttached,
		worktreePath,
		branchProblem,
		worktreeShortPath,
		agentListing,
		agentMissing,
		isRepo,
		canCreate,
		busy,
		error,
		statusBlurb,
		browse,
		submit,
	};
};
