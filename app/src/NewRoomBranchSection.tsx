import type { Dispatch, SetStateAction } from "react";
import { branchFieldAttachedAfterBlur } from "./branchName.ts";
import type { RepoStatus } from "./NewRoomDialogTypes.ts";

// Split out of `NewRoomDialog.tsx` (#461): the Branch field (mode cards,
// worktree branch name, base branch). Only rendered for a valid repo.

type ValidRepo = Extract<RepoStatus, { kind: "valid" }>;

export const BranchSection = ({
	repoStatus,
	branchMode,
	setBranchMode,
	branch,
	setBranch,
	setBranchAttached,
	worktreePath,
	branchProblem,
	worktreeShortPath,
	baseBranch,
	setBaseBranch,
	submit,
	onCancel,
}: {
	repoStatus: ValidRepo;
	branchMode: "worktree" | "current";
	setBranchMode: Dispatch<SetStateAction<"worktree" | "current">>;
	branch: string;
	setBranch: Dispatch<SetStateAction<string>>;
	setBranchAttached: Dispatch<SetStateAction<boolean>>;
	worktreePath: string;
	branchProblem: string | null;
	worktreeShortPath: string;
	baseBranch: string;
	setBaseBranch: Dispatch<SetStateAction<string>>;
	submit: () => Promise<void>;
	onCancel: () => void;
}) => (
	<div className="sk-field">
		<label>Branch</label>
		<div className="sk-radio-row">
			<div
				className={`sk-radio-card ${branchMode === "worktree" ? "selected" : ""}`}
				onClick={() => setBranchMode("worktree")}
			>
				<div className="top">New worktree</div>
				<div className="desc">own branch + folder</div>
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
			<div className="sk-field" style={{ marginTop: 6 }}>
				<label htmlFor="sk-worktree-branch">Worktree branch</label>
				<input
					id="sk-worktree-branch"
					className="sk-input"
					value={branch}
					title={worktreePath ? `→ ${worktreePath}` : undefined}
					onChange={(e) => {
						// Detach unconditionally, including on an edit to
						// empty — otherwise clear-and-retype would have the
						// proposal silently reappear before the next
						// keystroke landed (#227 review).
						setBranch(e.target.value);
						setBranchAttached(false);
					}}
					onBlur={(e) => {
						if (branchFieldAttachedAfterBlur(e.target.value)) setBranchAttached(true);
					}}
					onKeyDown={(e) => {
						if (e.key === "Enter") void submit();
						if (e.key === "Escape") onCancel();
					}}
				/>
				{(branchProblem || worktreeShortPath) && (
					<div
						style={{
							fontFamily: "var(--sk-mono)",
							fontSize: 10.5,
							marginTop: 2,
							color: branchProblem ? "var(--err)" : undefined,
						}}
					>
						{branchProblem ?? `→ ${worktreeShortPath}`}
					</div>
				)}
			</div>
		)}
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
				{(() => {
					const behind = repoStatus.branches.find((b) => b.name === baseBranch)?.behindUpstream;
					if (!behind) return null;
					return (
						<div
							style={{
								fontFamily: "var(--sk-mono)",
								fontSize: 10.5,
								marginTop: 4,
								lineHeight: 1.5,
								color: "var(--warn)",
							}}
						>
							{baseBranch} is {behind} commit{behind === 1 ? "" : "s"} behind its upstream (as of
							last fetch)
						</div>
					);
				})()}
			</div>
		)}
	</div>
);
