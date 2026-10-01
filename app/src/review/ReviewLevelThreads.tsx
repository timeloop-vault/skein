// Review-level comments (D5) — the remark about the change as a whole,
// which belongs to no file and no commit. Split out of ReviewPane.tsx
// (#460); the composer's open state stays in the pane.

import type { ThreadHandlers } from "./DiffBody.tsx";
import { Composer, ThreadView } from "./Thread.tsx";
import { type ReviewThread, addThread } from "./api.ts";

export const ReviewLevelThreads = ({
	roomId,
	cwd,
	reviewThreads,
	busy,
	run,
	handlers,
	composerOpen,
	setComposerOpen,
}: {
	roomId: string;
	cwd: string;
	reviewThreads: ReviewThread[];
	busy: boolean;
	run: (work: () => Promise<unknown>) => void;
	handlers: ThreadHandlers;
	composerOpen: boolean;
	setComposerOpen: (open: boolean) => void;
}) => (
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
		{composerOpen ? (
			<Composer
				placeholder="a comment on the whole review"
				busy={busy}
				submitLabel="comment"
				autoFocus
				onSubmit={(body) => {
					run(() => addThread(roomId, cwd, { scope: "review", anchorLines: [], body }));
					setComposerOpen(false);
				}}
				onCancel={() => setComposerOpen(false)}
			/>
		) : (
			<button
				type="button"
				className="rv-linkbtn"
				onClick={() => setComposerOpen(true)}
				disabled={busy}
			>
				+ comment on the whole review
			</button>
		)}
	</div>
);
