// The open file's strip above its diff (#212): path + harness chip,
// pending-scope accept/reject-all, "comment on file", the file-level
// composer, and the orphaned threads (D6). Split out of ReviewPane.tsx
// (#460); the composer's open state stays in the pane.

import { HChip } from "../components.tsx";
import type { HarnessKindOf } from "../harnessAttribution.ts";
import { acceptReview, rejectReview } from "../liveContext/review.ts";
import { addThread, type ReviewFile, type ReviewScope, type ReviewThread } from "./api.ts";
import type { ThreadHandlers } from "./DiffBody.tsx";
import { Composer, type DesignLink, ThreadView } from "./Thread.tsx";
import "./ReviewFileBar.css";

export const ReviewFileBar = ({
	roomId,
	cwd,
	scope,
	activeFile,
	orphans,
	busy,
	run,
	handlers,
	fileComposerOpen,
	setFileComposerOpen,
	harnessKindOf,
	designLinkFor,
}: {
	roomId: string;
	cwd: string;
	scope: ReviewScope;
	activeFile: ReviewFile | undefined;
	orphans: ReviewThread[];
	busy: boolean;
	run: (work: () => Promise<unknown>) => void;
	handlers: ThreadHandlers;
	fileComposerOpen: boolean;
	setFileComposerOpen: (open: boolean) => void;
	harnessKindOf: HarnessKindOf;
	designLinkFor: ((thread: ReviewThread) => DesignLink | undefined) | undefined;
}) => (
	<>
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
						design={t.scope === "element" ? designLinkFor?.(t) : undefined}
						onReply={(b) => handlers.onReply(t.id, b)}
						onResolve={(r) => handlers.onResolve(t.id, r)}
						onDelete={() => handlers.onDeleteThread(t.id)}
						onEditComment={handlers.onEditComment}
						onDeleteComment={handlers.onDeleteComment}
					/>
				))}
			</div>
		)}
	</>
);
