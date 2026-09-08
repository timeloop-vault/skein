// The unified diff, with hover-to-comment and threads rendered inline
// (#212, epic #52 D5/D6).
//
// One renderer for all three scopes: the backend hands every scope the
// same `ReviewHunk` shape, so branch, commit and pending differ only in
// where the hunks came from and whether the accept/reject verbs are
// offered.
//
// Selecting a range: the `+` in a line's gutter opens a composer on
// that line, and shift-clicking another `+` on the same side extends
// the anchor to cover both. One affordance, two behaviours, and no
// drag-select to fight the terminal beside it for pointer events.

import { useState } from "react";
import { HChip } from "../components.tsx";
import type { ReviewHunk } from "../liveContext/review.ts";
import type { HarnessKind } from "../types.ts";
import { Composer, ThreadView } from "./Thread.tsx";
import { type ReviewThread, type Side, threadsByLine } from "./api.ts";

/// A pending anchor: the lines the user has picked but not yet
/// commented on.
export interface LineSelection {
	side: Side;
	start: number;
	end: number;
	/** The rendered text of those lines — the anchor, captured at pick
	 *  time so an agent edit landing mid-compose cannot change it. */
	lines: string[];
}

const lineClass = (kind: ReviewHunk["lines"][number]["kind"]) =>
	kind === "add" ? "rv-line add" : kind === "delete" ? "rv-line del" : "rv-line";

/// Which side a diff line belongs to, and its number on that side.
///
/// A deleted line only exists in the old text and an added line only in
/// the new; a context line is in both, and anchors to the new side
/// because that is the text the next round of edits will move.
function anchorOf(line: ReviewHunk["lines"][number]): { side: Side; lineno: number } | null {
	if (line.kind === "delete") {
		return line.oldLineno == null ? null : { side: "old", lineno: line.oldLineno };
	}
	return line.newLineno == null ? null : { side: "new", lineno: line.newLineno };
}

export interface ThreadHandlers {
	onReply: (threadId: string, body: string) => void;
	onResolve: (threadId: string, resolved: boolean) => void;
	onDeleteThread: (threadId: string) => void;
	onEditComment: (commentId: string, body: string) => void;
	onDeleteComment: (commentId: string) => void;
}

const ThreadList = ({
	threads,
	busy,
	handlers,
}: {
	threads: ReviewThread[];
	busy: boolean;
	handlers: ThreadHandlers;
}) => (
	<>
		{threads.map((t) => (
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
	</>
);

export const DiffBody = ({
	hunks,
	threads,
	busy,
	owners,
	harnessKindOf,
	/** Accept/reject, offered only in the pending scope — the other two
	 *  scopes describe committed work, which those verbs cannot act on. */
	verbs,
	handlers,
	onComment,
}: {
	hunks: ReviewHunk[];
	threads: ReviewThread[];
	busy: boolean;
	owners: Array<string | undefined>;
	harnessKindOf: (harnessId: string) => HarnessKind;
	verbs?: { onAccept: (h: ReviewHunk) => void; onReject: (h: ReviewHunk) => void } | undefined;
	handlers: ThreadHandlers;
	onComment: (selection: LineSelection, body: string) => void;
}) => {
	const [selection, setSelection] = useState<LineSelection | null>(null);
	const byLine = threadsByLine(threads);

	const pick = (side: Side, lineno: number, content: string, extend: boolean) => {
		setSelection((prev) => {
			// Shift-click extends, but only within one side: an anchor
			// spanning both texts would have no single document to
			// re-match against.
			if (extend && prev && prev.side === side) {
				const start = Math.min(prev.start, lineno);
				const end = Math.max(prev.end, lineno);
				// The extended range's text is recovered from the rendered
				// hunks so the anchor stays exactly what is on screen.
				return { side, start, end, lines: linesBetween(hunks, side, start, end) };
			}
			return { side, start: lineno, end: lineno, lines: [content] };
		});
	};

	return (
		<>
			{hunks.map((h, hi) => {
				const owner = owners[hi];
				return (
					<div key={`${h.oldStart}-${h.newStart}-${hi}`}>
						<div className="rv-hunk-sep">
							{owner && <HChip kind={harnessKindOf(owner)} />}
							<span className="rv-hunk-range">{h.header}</span>
							{verbs && (
								<span className="rv-verbs">
									<button
										type="button"
										className="rv-act accept"
										disabled={busy}
										title="accept this hunk — advances the baseline, leaves the file as it is"
										onClick={() => verbs.onAccept(h)}
									>
										✓
									</button>
									<button
										type="button"
										className="rv-act reject"
										disabled={busy}
										title="reject this hunk — restores the baseline content on disk"
										onClick={() => verbs.onReject(h)}
									>
										↶
									</button>
								</span>
							)}
						</div>

						{h.lines.map((l, li) => {
							const at = anchorOf(l);
							const key = at ? `${at.side}:${at.lineno}` : null;
							const here = key ? (byLine.get(key) ?? []) : [];
							const selected =
								at != null &&
								selection != null &&
								selection.side === at.side &&
								at.lineno >= selection.start &&
								at.lineno <= selection.end;
							return (
								<div key={`${hi}-${li}`}>
									<div className={`${lineClass(l.kind)}${selected ? " picked" : ""}`}>
										<span className="gutter">
											<span className="ln">{l.oldLineno ?? ""}</span>
											<span className="ln">{l.newLineno ?? ""}</span>
											{at && (
												<button
													type="button"
													className="rv-add"
													title="comment on this line — shift-click another to extend"
													onClick={(e) => pick(at.side, at.lineno, l.content, e.shiftKey)}
												>
													+
												</button>
											)}
										</span>
										<span className="marker">
											{l.kind === "add" ? "+" : l.kind === "delete" ? "−" : ""}
										</span>
										<span className="src">{l.content}</span>
									</div>

									{here.length > 0 && (
										<div className="rv-inline">
											<ThreadList threads={here} busy={busy} handlers={handlers} />
										</div>
									)}

									{selected && at && selection.end === at.lineno && (
										<div className="rv-inline">
											<div className="rv-newthread">
												<div className="rv-newthread-head">
													{selection.start === selection.end
														? `line ${selection.start}`
														: `lines ${selection.start}–${selection.end}`}
													<span className="rv-side">{selection.side} side</span>
												</div>
												<Composer
													placeholder="leave a comment on these lines"
													busy={busy}
													submitLabel="comment"
													autoFocus
													onSubmit={(body) => {
														onComment(selection, body);
														setSelection(null);
													}}
													onCancel={() => setSelection(null)}
												/>
											</div>
										</div>
									)}
								</div>
							);
						})}
					</div>
				);
			})}
		</>
	);
};

/// The rendered text of `start..=end` on one side, read back out of the
/// hunks the user is looking at.
///
/// Gaps between hunks are simply absent rather than filled with
/// placeholder text: an anchor must be text that really exists, or
/// re-matching it would be matching against a fiction.
function linesBetween(hunks: ReviewHunk[], side: Side, start: number, end: number): string[] {
	const out: string[] = [];
	for (const h of hunks) {
		for (const l of h.lines) {
			const at = anchorOf(l);
			if (!at || at.side !== side) continue;
			if (at.lineno >= start && at.lineno <= end) out.push(l.content);
		}
	}
	return out;
}
