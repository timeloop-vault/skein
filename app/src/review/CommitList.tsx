// The review's commit list (#212, epic #52 D1).
//
// D1 scopes a review to a range but keeps per-commit granularity inside
// it, because that is how the work was written: the agent commits when
// it is done, and reading a five-commit branch commit by commit is
// often the only way its shape makes sense.
//
// Selecting a commit switches the diff to that commit alone. Commenting
// here attaches to the commit as a whole (D5) — the remark that is
// about the change rather than about any one line of it.

import type { ThreadHandlers } from "./DiffBody.tsx";
import { Composer, ThreadView, ago } from "./Thread.tsx";
import type { ReviewCommit, ReviewThread } from "./api.ts";

export const CommitList = ({
	commits,
	truncated,
	activeSha,
	threads,
	busy,
	handlers,
	onSelect,
	onComment,
}: {
	commits: ReviewCommit[];
	truncated: boolean;
	activeSha: string | undefined;
	/** Every commit-scoped thread in the review, filtered per row here. */
	threads: ReviewThread[];
	busy: boolean;
	handlers: ThreadHandlers;
	onSelect: (sha: string | undefined) => void;
	onComment: (sha: string, body: string) => void;
}) => {
	if (commits.length === 0) {
		return (
			<div className="rv-empty">
				nothing committed on this branch yet — the diff below is everything still uncommitted
			</div>
		);
	}
	return (
		<div className="rv-commits">
			{truncated && (
				<div className="rv-note">
					showing the most recent commits only — this range is longer than the review walks
				</div>
			)}
			{commits.map((c) => {
				const mine = threads.filter((t) => t.commitSha === c.sha);
				const active = c.sha === activeSha;
				return (
					<div key={c.sha} className={`rv-commit${active ? " active" : ""}`}>
						<button
							type="button"
							className="rv-commit-main"
							// Clicking the open commit closes it, which is how the
							// user gets back to the whole-branch diff without
							// hunting for a separate "all" control.
							onClick={() => onSelect(active ? undefined : c.sha)}
							title={c.body || c.summary}
						>
							<span className="rv-sha">{c.shortSha}</span>
							<span className="rv-commit-summary">{c.summary}</span>
							{c.isMerge && (
								<span className="rv-merge" title="a merge — shown against its first parent">
									merge
								</span>
							)}
							{c.threadCount > 0 && <span className="rv-badge open">{c.threadCount}</span>}
							<span className="rv-time">{ago(c.timeMs)}</span>
						</button>
						{active && (
							<div className="rv-commit-detail">
								{c.body && <pre className="rv-commit-body">{c.body}</pre>}
								<div className="rv-commit-author">{c.authorName}</div>
								{mine.map((t) => (
									<ThreadView
										key={t.id}
										thread={t}
										busy={busy}
										onReply={(body) => handlers.onReply(t.id, body)}
										onResolve={(resolved) => handlers.onResolve(t.id, resolved)}
										onDelete={() => handlers.onDeleteThread(t.id)}
										onEditComment={handlers.onEditComment}
										onDeleteComment={handlers.onDeleteComment}
									/>
								))}
								<Composer
									placeholder={`comment on ${c.shortSha} as a whole`}
									busy={busy}
									submitLabel="comment"
									onSubmit={(body) => onComment(c.sha, body)}
								/>
							</div>
						)}
					</div>
				);
			})}
		</div>
	);
};
