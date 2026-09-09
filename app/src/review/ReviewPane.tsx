// The review surface (#212, epic #52 — sub-issue B).
//
// What #52 is for: in a Skein room all code is written by an agent and
// reviewed by the person who directed it, and the GitHub round-trip
// exists to share work between *people*. With one author and one
// reviewer who are the same person it is mostly ceremony. So the review
// happens here, in the room, beside the agent that wrote the code.
//
// GitHub-shaped, because that is the surface the user already reviews
// in: base picker, commit list, file list, per-file diff, comments
// inline. Narrowed to one column because it shares the window with the
// harness terminal — the whole point is reading the diff and talking to
// the agent without switching away from either.
//
// This replaces the Diff card (#211) rather than running beside it, per
// #52: one diff renderer, three scopes. Accept and reject move here,
// into the pending scope, which is the only scope they can act on.
//
// Sub-issue C (#213) has landed: an agent reads and answers these
// comments over the MCP endpoint in `agent_api/`. Two consequences
// visible here — a comment can be authored by a harness (rendered with
// its byline), and a thread can carry the agent's "addressed" claim,
// which sits *beside* the resolve control rather than replacing it.
// Resolve stays the reviewer's, and the API has no verb for it (D8).
//
// It also means this pane can change while nobody touches a file, so
// `useAgentWrites` listens for the backend's `skein://review-changed`
// alongside the worktree watcher.

import { useCallback, useEffect, useMemo, useState } from "react";
import { HChip } from "../components.tsx";
import { acceptReview, attributeHunks, rejectReview } from "../liveContext/review.ts";
import type { ReviewHunk } from "../liveContext/review.ts";
import type { HarnessAction } from "../liveContext/store.ts";
import type { HarnessKind } from "../types.ts";
import { CommitList } from "./CommitList.tsx";
import { DiffBody, type LineSelection, type ThreadHandlers } from "./DiffBody.tsx";
import { FileList } from "./FileList.tsx";
import { SignoffControl, SignoffNotice } from "./SignoffControl.tsx";
import { Composer, ThreadView } from "./Thread.tsx";
import {
	type ReviewFile,
	type ReviewScope,
	addThread,
	contributingHarnesses,
	deleteComment,
	deleteThread,
	editComment,
	markViewed,
	replyToThread,
	resolveThread,
	setBaseRef,
	unplacedThreads,
} from "./api.ts";
import {
	useAgentWrites,
	useReviewFile,
	useReviewScope,
	useSignoff,
	useWorktreeWatcher,
} from "./useReviewData.ts";
import "./review.css";

const SCOPES: Array<{ id: ReviewScope; label: string; title: string }> = [
	{
		id: "branch",
		label: "branch",
		title: "everything this branch does — committed and uncommitted, against the base",
	},
	{ id: "commit", label: "commits", title: "read the branch one commit at a time" },
	{
		id: "pending",
		label: "pending",
		title: "uncommitted work only — the one scope you can accept or reject",
	},
];

export const ReviewPane = ({
	roomId,
	cwd,
	actions,
	harnessKindOf,
	visible,
}: {
	roomId: string;
	cwd: string;
	/** The room's action rows — per-hunk harness attribution only (D4). */
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	visible: boolean;
}) => {
	const [scope, setScope] = useState<ReviewScope>("branch");
	const [commitSha, setCommitSha] = useState<string | undefined>(undefined);
	const [activePath, setActivePath] = useState<string | undefined>(undefined);
	const [busy, setBusy] = useState(false);
	const [actionError, setActionError] = useState<string | undefined>(undefined);
	// Bumped after every mutation so the open file re-pulls its threads;
	// comments touch only sqlite and raise no filesystem event.
	const [nonce, setNonce] = useState(0);
	const [showFiles, setShowFiles] = useState(true);
	const [reviewComposerOpen, setReviewComposerOpen] = useState(false);
	const [fileComposerOpen, setFileComposerOpen] = useState(false);
	const [harnessFilter, setHarnessFilter] = useState<string | undefined>(undefined);

	const effectiveCommit = scope === "commit" ? commitSha : undefined;
	const {
		data,
		error,
		loading,
		refresh: refreshScope,
		setData,
	} = useReviewScope(roomId, cwd, scope, effectiveCommit, visible);

	const bump = useCallback(() => setNonce((n) => n + 1), []);
	const refreshAll = useCallback(() => {
		refreshScope();
		bump();
	}, [refreshScope, bump]);
	useWorktreeWatcher(cwd, visible, refreshAll);
	useAgentWrites(roomId, visible, refreshAll);

	// #214: the reviewer's sign-off. Keyed off the same nonce as
	// everything else, which is what makes it lapse on its own — a
	// commit refreshes the pane, and the refresh is when the approval
	// stops matching HEAD.
	const {
		status: signoff,
		error: signoffError,
		busy: signoffBusy,
		set: setSignoffState,
	} = useSignoff(roomId, cwd, visible, nonce);

	const { file, error: fileError } = useReviewFile(
		roomId,
		cwd,
		activePath,
		scope,
		effectiveCommit,
		visible,
		nonce,
	);

	const allFiles = useMemo(() => data?.files ?? [], [data]);
	// D4's filter: a filter over one diff, never a partition of it.
	const harnesses = useMemo(() => contributingHarnesses(allFiles), [allFiles]);
	const files = useMemo(
		() => (harnessFilter ? allFiles.filter((f) => f.harnessId === harnessFilter) : allFiles),
		[allFiles, harnessFilter],
	);

	// A filter whose harness owns nothing here any more would hide the
	// whole list; drop it rather than show an empty pane.
	useEffect(() => {
		if (harnessFilter && !harnesses.includes(harnessFilter)) setHarnessFilter(undefined);
	}, [harnessFilter, harnesses]);

	// Keep the selection honest: a file that leaves the list (accepted,
	// or gone from this scope) must not leave a stale diff on screen.
	useEffect(() => {
		if (activePath && !files.some((f) => f.path === activePath)) {
			setActivePath(undefined);
		}
	}, [files, activePath]);

	const run = (work: () => Promise<unknown>) => {
		setBusy(true);
		setActionError(undefined);
		work()
			.catch((err: unknown) => {
				setActionError(err instanceof Error ? err.message : String(err));
			})
			.finally(() => {
				setBusy(false);
				// Always re-fetch, including after a failure: the usual
				// reason a call fails is that the file moved, and the user
				// needs to see what it moved to.
				refreshAll();
			});
	};

	const handlers: ThreadHandlers = {
		onReply: (threadId, body) => run(() => replyToThread(roomId, threadId, body)),
		onResolve: (threadId, resolved) => run(() => resolveThread(threadId, resolved)),
		onDeleteThread: (threadId) => run(() => deleteThread(threadId)),
		onEditComment: (commentId, body) => run(() => editComment(commentId, body)),
		onDeleteComment: (commentId) => run(() => deleteComment(commentId)),
	};

	const commentOnLines = (selection: LineSelection, body: string) => {
		if (!activePath) return;
		run(() =>
			addThread(roomId, cwd, {
				scope: "line",
				filePath: activePath,
				commitSha: effectiveCommit,
				side: selection.side,
				lineStart: selection.start,
				lineEnd: selection.end,
				anchorLines: selection.lines,
				body,
			}),
		);
	};

	const toggleViewed = (f: ReviewFile) =>
		run(() => markViewed(roomId, f.path, f.contentHash, !f.viewed));

	// Per-hunk harness attribution, from #211. Only meaningful in the
	// pending scope: a patch row's line numbers are from the moment of
	// that edit, and a committed range has moved on from them.
	const owners = useMemo(() => {
		if (scope !== "pending" || !file) return [];
		return attributeHunks(actions, {
			path: file.path,
			hunks: file.hunks,
			harnessId: files.find((f) => f.path === file.path)?.harnessId ?? "",
		});
	}, [scope, file, files, actions]);

	const verbs =
		scope === "pending" && file
			? {
					onAccept: (h: ReviewHunk) =>
						run(() => acceptReview(roomId, cwd, file.path, [h], undefined)),
					onReject: (h: ReviewHunk) =>
						run(() => rejectReview(roomId, cwd, file.path, [h], undefined)),
				}
			: undefined;

	const activeFile = files.find((f) => f.path === activePath);
	const orphans = file ? unplacedThreads(file.threads) : [];
	const reviewThreads = data?.threads ?? [];

	// ── header ────────────────────────────────────────────────────

	const header = (
		<div className="rv-head">
			<div className="rv-head-row">
				{data?.isRepo ? (
					<label className="rv-base">
						<span className="rv-base-label">base</span>
						<select
							className="rv-select"
							value={data.baseRef ?? ""}
							disabled={busy}
							onChange={(e) => {
								const next = e.target.value;
								if (!next) return;
								run(() => setBaseRef(roomId, cwd, next).then(setData));
							}}
						>
							{/* A base that no longer resolves still shows, so the
							    header explains the empty review rather than
							    silently swapping in a different branch. */}
							{data.baseRef && !data.branches.includes(data.baseRef) && (
								<option value={data.baseRef}>{data.baseRef}</option>
							)}
							{data.branches.map((b) => (
								<option key={b} value={b}>
									{b}
								</option>
							))}
						</select>
					</label>
				) : (
					<span className="rv-nonrepo">not a git repository</span>
				)}
				<span className="rv-counts">
					{data?.headBranch && <span className="rv-branch">{data.headBranch}</span>}
					{data && data.commits.length > 0 && (
						<span>
							{data.commits.length} commit{data.commits.length === 1 ? "" : "s"}
						</span>
					)}
					{data && data.files.length > 0 && <span>{data.files.length} files</span>}
					{data && data.additions > 0 && <span className="delta-add">+{data.additions}</span>}
					{data && data.deletions > 0 && <span className="delta-del">−{data.deletions}</span>}
				</span>
			</div>

			<div className="rv-head-row">
				<span className="rv-scopes">
					{SCOPES.map((s) => (
						<button
							type="button"
							key={s.id}
							className={`rv-scope${scope === s.id ? " on" : ""}`}
							title={s.title}
							onClick={() => {
								setScope(s.id);
								setActivePath(undefined);
							}}
						>
							{s.label}
							{s.id === "pending" && data && data.pendingCount > 0 && (
								<span className="rv-badge open">{data.pendingCount}</span>
							)}
						</button>
					))}
				</span>
				<span className="rv-head-right">
					{data && data.unresolvedCount > 0 && (
						<span className="rv-unresolved" title="unresolved comment threads in this review">
							{data.unresolvedCount} open
						</span>
					)}
					{scope === "pending" && files.length > 0 && (
						<button
							type="button"
							className="rv-act all"
							disabled={busy}
							title="accept every pending file — advances the baseline, writes nothing"
							onClick={() => run(() => acceptReview(roomId, cwd, undefined, [], undefined))}
						>
							✓ all
						</button>
					)}
					{/* #214: the terminal act of a review. Not a merge and not
					    a push — Skein does neither. This records that the
					    reviewer approved this commit, which is what the agent
					    checks before landing the branch its own way. */}
					<SignoffControl status={signoff} busy={signoffBusy} onSet={setSignoffState} />
				</span>
			</div>
		</div>
	);

	// ── body ──────────────────────────────────────────────────────

	let body: React.ReactNode;
	if (scope === "commit" && !commitSha) {
		body = (
			<CommitList
				commits={data?.commits ?? []}
				truncated={data?.truncated ?? false}
				activeSha={commitSha}
				threads={reviewThreads}
				busy={busy}
				handlers={handlers}
				onSelect={setCommitSha}
				onComment={(sha, cbody) =>
					run(() =>
						addThread(roomId, cwd, {
							scope: "commit",
							commitSha: sha,
							anchorLines: [],
							body: cbody,
						}),
					)
				}
			/>
		);
	} else if (files.length === 0) {
		body = (
			<div className="rv-empty">
				{loading
					? "reading the review…"
					: scope === "pending"
						? "nothing uncommitted — when an agent edits a file, it appears here"
						: data?.isRepo === false
							? "this room's folder is not a git repository, so there is no branch to review"
							: "nothing on this branch yet"}
			</div>
		);
	} else if (!file) {
		body = <div className="rv-empty">select a file to review it</div>;
	} else if (file.blocked) {
		body = <div className="rv-empty">{file.blocked} — no line diff to show</div>;
	} else if (file.hunks.length === 0) {
		body = (
			<div className="rv-empty">
				no diff in this scope
				{orphans.length > 0 && " — its comments are below"}
			</div>
		);
	} else {
		body = (
			<DiffBody
				hunks={file.hunks}
				threads={file.threads}
				busy={busy}
				owners={owners}
				harnessKindOf={harnessKindOf}
				verbs={verbs}
				handlers={handlers}
				onComment={commentOnLines}
			/>
		);
	}

	return (
		<div className="rv">
			{header}

			{(actionError ?? error ?? fileError ?? signoffError ?? data?.error) && (
				<div className="rv-err">
					{actionError ?? error ?? fileError ?? signoffError ?? data?.error}
				</div>
			)}

			{/* A lapsed sign-off is the one state the reviewer has to see
			    rather than hover over: their approval stopped covering the
			    branch the moment the agent committed again. */}
			<SignoffNotice status={signoff} />

			{scope === "commit" && commitSha && (
				<button type="button" className="rv-backlink" onClick={() => setCommitSha(undefined)}>
					← all commits
				</button>
			)}

			{files.length > 0 && (
				<div className="rv-section">
					<div className="rv-section-bar">
						<button
							type="button"
							className="rv-section-head"
							onClick={() => setShowFiles(!showFiles)}
						>
							<span className="chev">{showFiles ? "▾" : "▸"}</span>
							<span>files</span>
							<span className="rv-section-count">{files.length}</span>
						</button>
						{/* D4: harness is a chip and a filter, never a partition.
						    Shown only once more than one harness has touched the
						    review — with one, it is a control that does nothing. */}
						{harnesses.length > 1 && (
							<span className="rv-filter">
								<button
									type="button"
									className={`rv-chip${harnessFilter === undefined ? " on" : ""}`}
									onClick={() => setHarnessFilter(undefined)}
									title="show every harness's files"
								>
									all
								</button>
								{harnesses.map((h) => (
									<button
										type="button"
										key={h}
										className={`rv-chip${harnessFilter === h ? " on" : ""}`}
										onClick={() => setHarnessFilter(harnessFilter === h ? undefined : h)}
										title={`only files last written by ${harnessKindOf(h)}`}
									>
										<HChip kind={harnessKindOf(h)} />
									</button>
								))}
							</span>
						)}
					</div>
					{showFiles && (
						<FileList
							files={files}
							activePath={activePath}
							harnessKindOf={harnessKindOf}
							onSelect={setActivePath}
							onToggleViewed={toggleViewed}
						/>
					)}
				</div>
			)}

			{activeFile && (
				<div className="rv-filebar">
					<span className="rv-filebar-path" title={activeFile.path}>
						{activeFile.path}
					</span>
					{activeFile.harnessId && <HChip kind={harnessKindOf(activeFile.harnessId)} />}
					{scope === "pending" && (
						<span className="rv-verbs">
							<button
								type="button"
								className="rv-act accept"
								disabled={busy}
								title={`accept all of ${activeFile.name} — advances the baseline, leaves the file as it is`}
								onClick={() =>
									run(() => acceptReview(roomId, cwd, activeFile.path, [], activeFile.contentHash))
								}
							>
								✓
							</button>
							<button
								type="button"
								className="rv-act reject"
								disabled={busy}
								title={`reject all of ${activeFile.name} — restores the baseline content on disk`}
								onClick={() =>
									run(() => rejectReview(roomId, cwd, activeFile.path, [], activeFile.contentHash))
								}
							>
								↶
							</button>
						</span>
					)}
					<button
						type="button"
						className="rv-linkbtn"
						disabled={busy}
						title="comment on this file as a whole"
						onClick={() => setFileComposerOpen(true)}
					>
						comment on file
					</button>
				</div>
			)}

			{activeFile && fileComposerOpen && (
				<div className="rv-orphans">
					<Composer
						placeholder={`a comment on ${activeFile.name} as a whole`}
						busy={busy}
						submitLabel="comment"
						autoFocus
						onSubmit={(body) => {
							run(() =>
								addThread(roomId, cwd, {
									scope: "file",
									filePath: activeFile.path,
									anchorLines: [],
									body,
								}),
							);
							setFileComposerOpen(false);
						}}
						onCancel={() => setFileComposerOpen(false)}
					/>
				</div>
			)}

			{/* Threads with nowhere to sit — file-scoped ones, and every
			    line thread the matcher could not place. Above the diff so
			    an orphaned comment is impossible to miss (D6). */}
			{orphans.length > 0 && (
				<div className="rv-orphans">
					{orphans.map((t) => (
						<ThreadView
							key={t.id}
							thread={t}
							busy={busy}
							onReply={(b) => handlers.onReply(t.id, b)}
							onResolve={(r) => handlers.onResolve(t.id, r)}
							onDelete={() => handlers.onDeleteThread(t.id)}
							onEditComment={handlers.onEditComment}
							onDeleteComment={handlers.onDeleteComment}
						/>
					))}
				</div>
			)}

			<div className="rv-body">{body}</div>

			{/* Review-level comments (D5) — the remark about the change as
			    a whole, which belongs to no file and no commit. */}
			<div className="rv-review-threads">
				{reviewThreads
					.filter((t) => t.scope === "review")
					.map((t) => (
						<ThreadView
							key={t.id}
							thread={t}
							busy={busy}
							onReply={(b) => handlers.onReply(t.id, b)}
							onResolve={(r) => handlers.onResolve(t.id, r)}
							onDelete={() => handlers.onDeleteThread(t.id)}
							onEditComment={handlers.onEditComment}
							onDeleteComment={handlers.onDeleteComment}
						/>
					))}
				{reviewComposerOpen ? (
					<Composer
						placeholder="a comment on the whole review"
						busy={busy}
						submitLabel="comment"
						autoFocus
						onSubmit={(body) => {
							run(() => addThread(roomId, cwd, { scope: "review", anchorLines: [], body }));
							setReviewComposerOpen(false);
						}}
						onCancel={() => setReviewComposerOpen(false)}
					/>
				) : (
					<button
						type="button"
						className="rv-linkbtn"
						onClick={() => setReviewComposerOpen(true)}
						disabled={busy}
					>
						+ comment on the whole review
					</button>
				)}
			</div>
		</div>
	);
};
