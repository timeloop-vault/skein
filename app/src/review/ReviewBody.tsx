// What fills the review pane's body for the current scope (#212): the
// commit list, an empty/hint message, or one file's diff (image, blocked,
// no-diff or the line diff). Split out of ReviewPane.tsx (#460) — pure
// render of what the orchestrator hands it, no state of its own.

import type { ReviewHunk } from "../liveContext/review.ts";
import type { HarnessKind } from "../types.ts";
import { CommitList } from "./CommitList.tsx";
import { DiffBody, type LineSelection, type ThreadHandlers } from "./DiffBody.tsx";
import { ImageDiff } from "./ImageDiff.tsx";
import {
	type ReviewFileDetail,
	type ReviewScope,
	type ReviewScopeData,
	type ReviewThread,
	addThread,
} from "./api.ts";
import { type SvgViewMode, chooseReviewBody, isSvgPath } from "./imageDiffModel.ts";

export const ReviewBody = ({
	roomId,
	cwd,
	scope,
	commitSha,
	effectiveCommit,
	setCommitSha,
	data,
	loading,
	filesCount,
	file,
	orphanCount,
	reviewThreads,
	busy,
	run,
	handlers,
	svgMode,
	setSvgMode,
	owners,
	harnessKindOf,
	verbs,
	commentOnLines,
}: {
	roomId: string;
	cwd: string;
	scope: ReviewScope;
	commitSha: string | undefined;
	effectiveCommit: string | undefined;
	setCommitSha: (sha: string | undefined) => void;
	data: ReviewScopeData | undefined;
	loading: boolean;
	filesCount: number;
	file: ReviewFileDetail | undefined;
	orphanCount: number;
	reviewThreads: ReviewThread[];
	busy: boolean;
	run: (work: () => Promise<unknown>) => void;
	handlers: ThreadHandlers;
	svgMode: SvgViewMode;
	setSvgMode: (mode: SvgViewMode) => void;
	owners: Array<string | undefined>;
	harnessKindOf: (harnessId: string) => HarnessKind;
	verbs: { onAccept: (h: ReviewHunk) => void; onReject: (h: ReviewHunk) => void } | undefined;
	commentOnLines: (selection: LineSelection, body: string) => void;
}) => {
	if (scope === "commit" && !commitSha) {
		return (
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
	}
	if (filesCount === 0) {
		return (
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
	}
	if (!file) return <div className="rv-empty">select a file to review it</div>;

	// #409: an image gets a before/after instead of the "no line diff"
	// message a binary/toolarge block would otherwise show. SVG is
	// also text, so it gets a toggle between the two rather than only
	// ever the image — comments stay attached to the text view, which
	// is why the toggle lives beside the body rather than replacing it.
	const rb = chooseReviewBody({
		hasFile: true,
		path: file.path,
		blocked: file.blocked,
		hunksLength: file.hunks.length,
		svgMode,
	});
	let content: React.ReactNode;
	switch (rb.kind) {
		case "image":
			content = (
				<ImageDiff
					roomId={roomId}
					cwd={cwd}
					scope={scope}
					commitSha={effectiveCommit}
					path={file.path}
					change={file.change}
					contentHash={file.contentHash}
				/>
			);
			break;
		case "blocked":
			content = <div className="rv-empty">{rb.blocked} — no line diff to show</div>;
			break;
		case "no-diff":
			content = (
				<div className="rv-empty">
					{file.change === "unchanged" ? "no changes in this file" : "no diff in this scope"}
					{orphanCount > 0 && " — its comments are above"}
				</div>
			);
			break;
		default:
			content = (
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
		<>
			{isSvgPath(file.path) && (
				<div className="rv-imgtoggle">
					<button
						type="button"
						className={`rv-btn${svgMode === "image" ? " primary" : ""}`}
						onClick={() => setSvgMode("image")}
					>
						image
					</button>
					<button
						type="button"
						className={`rv-btn${svgMode === "text" ? " primary" : ""}`}
						onClick={() => setSvgMode("text")}
					>
						text
					</button>
				</div>
			)}
			{content}
		</>
	);
};
