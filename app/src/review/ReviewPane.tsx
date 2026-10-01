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
import { acceptReview, attributeHunks, rejectReview } from "../liveContext/review.ts";
import type { ReviewHunk } from "../liveContext/review.ts";
import type { HarnessAction } from "../liveContext/store.ts";
import type { Harness, HarnessKind } from "../types.ts";
import type { LineSelection, ThreadHandlers } from "./DiffBody.tsx";
import { ReviewBody } from "./ReviewBody.tsx";
import { ReviewFileBar } from "./ReviewFileBar.tsx";
import { ReviewFileSection } from "./ReviewFileSection.tsx";
import { ReviewHeader } from "./ReviewHeader.tsx";
import { ReviewLevelThreads } from "./ReviewLevelThreads.tsx";
import { SignoffConfirm, type SignoffIntent, SignoffNotice } from "./SignoffControl.tsx";
import type { DesignLink } from "./Thread.tsx";
import {
	type ReviewFile,
	type ReviewScope,
	type ReviewThread,
	addThread,
	contributingHarnesses,
	deleteComment,
	deleteThread,
	editComment,
	markViewed,
	replyToThread,
	resolveThread,
	unplacedThreads,
} from "./api.ts";
import type { SvgViewMode } from "./imageDiffModel.ts";
import {
	useAgentWrites,
	useReviewFile,
	useReviewScope,
	useSignoff,
	useWorktreeWatcher,
} from "./useReviewData.ts";
import { useReviewNudge } from "./useReviewNudge.ts";
import "./ReviewPane.css";

export const ReviewPane = ({
	roomId,
	cwd,
	actions,
	harnessKindOf,
	activeHarness,
	visible,
	designLinkFor,
}: {
	/** "Show in design pane" for an element thread (#434); absent = no link. */
	designLinkFor?: ((thread: ReviewThread) => DesignLink | undefined) | undefined;
	roomId: string;
	cwd: string;
	/** The room's action rows — per-hunk harness attribution only (D4). */
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	/** The room's currently active/focused harness — the #238 nudge
	 *  target. No picker: a nudge always goes to whichever harness the
	 *  user would otherwise be typing into. */
	activeHarness: Harness | undefined;
	visible: boolean;
}) => {
	const [scope, setScope] = useState<ReviewScope>("branch");
	const [commitSha, setCommitSha] = useState<string | undefined>(undefined);
	const [activePath, setActivePath] = useState<string | undefined>(undefined);
	// The SVG image/text toggle (#409) — per file, so switching files
	// doesn't carry one file's choice onto the next.
	const [svgMode, setSvgMode] = useState<SvgViewMode>("image");
	const [busy, setBusy] = useState(false);
	const [actionError, setActionError] = useState<string | undefined>(undefined);
	// Bumped after every mutation so the open file re-pulls its threads;
	// comments touch only sqlite and raise no filesystem event.
	const [nonce, setNonce] = useState(0);
	const [showFiles, setShowFiles] = useState(true);
	const [reviewComposerOpen, setReviewComposerOpen] = useState(false);
	const [fileComposerOpen, setFileComposerOpen] = useState(false);
	const [harnessFilter, setHarnessFilter] = useState<string | undefined>(undefined);
	// Which sign-off confirmation is open. Here rather than in the
	// control, because the button is in the header and its confirmation
	// renders below it.
	const [signoffIntent, setSignoffIntent] = useState<SignoffIntent | undefined>(undefined);

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

	// #238: the Nudge button — see `useReviewNudge`.
	const { activeCapabilities, nudge, nudgeGate, nudgeDisabledReason, onNudge } = useReviewNudge(
		activeHarness,
		signoff,
		data?.unresolvedCount ?? 0,
		setActionError,
	);

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

	// biome-ignore lint/correctness/useExhaustiveDependencies: `activePath` is the deliberate trigger — a new file starts the toggle over on "image".
	useEffect(() => {
		setSvgMode("image");
	}, [activePath]);

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
		onResolve: (threadId, resolved) => run(() => resolveThread(roomId, threadId, resolved)),
		onDeleteThread: (threadId) => run(() => deleteThread(roomId, threadId)),
		onEditComment: (commentId, body) => run(() => editComment(roomId, commentId, body)),
		onDeleteComment: (commentId) => run(() => deleteComment(roomId, commentId)),
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
		<ReviewHeader
			roomId={roomId}
			cwd={cwd}
			data={data}
			setData={setData}
			busy={busy}
			run={run}
			scope={scope}
			setScope={setScope}
			setActivePath={setActivePath}
			filesCount={files.length}
			activeCapabilities={activeCapabilities}
			nudge={nudge}
			nudgeGate={nudgeGate}
			nudgeDisabledReason={nudgeDisabledReason}
			onNudge={onNudge}
			signoff={signoff}
			signoffBusy={signoffBusy}
			signoffIntent={signoffIntent}
			setSignoffIntent={setSignoffIntent}
		/>
	);

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

			<SignoffConfirm
				status={signoff}
				pending={signoffIntent}
				busy={signoffBusy}
				onConfirm={(approved) => {
					setSignoffState(approved);
					setSignoffIntent(undefined);
				}}
				onCancel={() => setSignoffIntent(undefined)}
			/>

			{scope === "commit" && commitSha && (
				<button type="button" className="rv-backlink" onClick={() => setCommitSha(undefined)}>
					← all commits
				</button>
			)}

			{files.length > 0 && (
				<ReviewFileSection
					files={files}
					harnesses={harnesses}
					harnessFilter={harnessFilter}
					setHarnessFilter={setHarnessFilter}
					showFiles={showFiles}
					setShowFiles={setShowFiles}
					activePath={activePath}
					setActivePath={setActivePath}
					harnessKindOf={harnessKindOf}
					toggleViewed={toggleViewed}
				/>
			)}

			<ReviewFileBar
				roomId={roomId}
				cwd={cwd}
				scope={scope}
				activeFile={activeFile}
				orphans={orphans}
				busy={busy}
				run={run}
				handlers={handlers}
				fileComposerOpen={fileComposerOpen}
				setFileComposerOpen={setFileComposerOpen}
				harnessKindOf={harnessKindOf}
				designLinkFor={designLinkFor}
			/>

			<div className="rv-body">
				<ReviewBody
					roomId={roomId}
					cwd={cwd}
					scope={scope}
					commitSha={commitSha}
					effectiveCommit={effectiveCommit}
					setCommitSha={setCommitSha}
					data={data}
					loading={loading}
					filesCount={files.length}
					file={file}
					orphanCount={orphans.length}
					reviewThreads={reviewThreads}
					busy={busy}
					run={run}
					handlers={handlers}
					svgMode={svgMode}
					setSvgMode={setSvgMode}
					owners={owners}
					harnessKindOf={harnessKindOf}
					verbs={verbs}
					commentOnLines={commentOnLines}
				/>
			</div>

			<ReviewLevelThreads
				roomId={roomId}
				cwd={cwd}
				reviewThreads={reviewThreads}
				busy={busy}
				run={run}
				handlers={handlers}
				composerOpen={reviewComposerOpen}
				setComposerOpen={setReviewComposerOpen}
			/>
		</div>
	);
};
