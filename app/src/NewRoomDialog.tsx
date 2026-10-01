import { BranchSection } from "./NewRoomBranchSection.tsx";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import { HarnessSection } from "./NewRoomHarnessSection.tsx";
import type { DefaultAgents, FolderDefaults, NewRoomMemory, RecentFolder } from "./prefs.ts";
import { useFocusRestore } from "./useFocusRestore.ts";
import { useNewRoomForm } from "./useNewRoomForm.tsx";
import "./modal.css";
import "./NewRoomDialog.css";

// ── New room dialog ────────────────────────────────────────────────
// The picked folder becomes the room's cwd; every harness in the
// room spawns into it. "New worktree" mode resolves to a fresh
// libgit2 worktree path; "Current branch" mode uses the picked path
// as-is.
//
// Split out of App.tsx, then split again (#19): the DTO types live in
// `NewRoomDialogTypes.ts`, the Agent-field note in
// `NewRoomAgentFieldNote.tsx`, and the folder-inspection / branch-
// validation state machine in the `useNewRoomForm` hook — this file is
// the JSX.

export type { CreateRoomArgs, FolderInfoDto } from "./NewRoomDialogTypes.ts";

export const NewRoomDialog = ({
	defaultCwd,
	initialCwd,
	initialDefaults,
	defaultAgents,
	memory,
	appBranchTemplate,
	recent,
	onRemember,
	onCommit,
	onCancel,
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
	/** Known folders, MRU-first, for the Folder dropdown (#233). */
	recent: RecentFolder[];
	/** Called with the folder and its defaults after a room is created. */
	onRemember: (folder: string, defaults: Omit<FolderDefaults, "lastUsed">) => void;
	onCommit: (args: CreateRoomArgs) => void;
	onCancel: () => void;
}) => {
	useFocusRestore();
	const {
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
		canCreate,
		busy,
		error,
		statusBlurb,
		browse,
		submit,
	} = useNewRoomForm({
		defaultCwd,
		initialCwd,
		initialDefaults,
		defaultAgents,
		memory,
		appBranchTemplate,
		onRemember,
		onCommit,
	});

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
						{/* The row is the menu's positioning context, so the dropdown
						    spans the whole field rather than hanging off the caret —
						    a menu only as wide as its trigger squeezed the paths it
						    exists to show (#233). */}
						<div
							className="sk-folder-row"
							ref={recentRef}
							onKeyDown={(e) => {
								// On the row, not the caret: once the menu is open focus
								// is on one of its rows, and Escape has to close it from
								// there too.
								if (e.key === "Escape" && showRecent) {
									e.stopPropagation();
									setShowRecent(false);
								}
							}}
						>
							<input
								className="sk-input"
								style={{ flex: 1 }}
								placeholder="Pick a folder…"
								value={cwd}
								// The menu overlays the status blurb, so it gets out of
								// the way the moment the field is being used directly.
								onFocus={() => setShowRecent(false)}
								onChange={(e) => {
									setResolvedFromWorktree(false);
									setCwd(e.target.value);
								}}
							/>
							{recent.length > 0 && (
								<button
									className="sk-btn"
									type="button"
									aria-haspopup="menu"
									aria-expanded={showRecent}
									title="Recent folders"
									onClick={() => setShowRecent((v) => !v)}
								>
									▾
								</button>
							)}
							<button className="sk-btn" onClick={browse} type="button">
								Browse…
							</button>
							{showRecent && (
								<div className="sk-recent-menu" role="menu">
									{recent.map((r) => (
										<button
											key={r.folder}
											className="sk-recent-row"
											role="menuitem"
											type="button"
											title={r.folder}
											onClick={() => pickRecent(r)}
										>
											<span className="path">{r.folder}</span>
											{r.defaults.baseBranch && (
												<span className="branch">{r.defaults.baseBranch}</span>
											)}
										</button>
									))}
								</div>
							)}
						</div>
						{statusBlurb && (
							<div style={{ fontFamily: "var(--sk-mono)", fontSize: 10.5, marginTop: 2 }}>
								{statusBlurb}
							</div>
						)}
					</div>

					{repoStatus.kind === "valid" && (
						<BranchSection
							repoStatus={repoStatus}
							branchMode={branchMode}
							setBranchMode={setBranchMode}
							branch={branch}
							setBranch={setBranch}
							setBranchAttached={setBranchAttached}
							worktreePath={worktreePath}
							branchProblem={branchProblem}
							worktreeShortPath={worktreeShortPath}
							baseBranch={baseBranch}
							setBaseBranch={setBaseBranch}
							submit={submit}
							onCancel={onCancel}
						/>
					)}

					<HarnessSection
						harness={harness}
						setHarness={setHarness}
						agent={agent}
						setAgent={setAgent}
						agentListing={agentListing}
						agentMissing={agentMissing}
						defaultAgents={defaultAgents}
					/>

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
