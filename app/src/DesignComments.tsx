// DesignComments — the side list of element threads next to a design
// preview (#434), plus the composer for a freshly picked element. Every
// string here came from the page or the comment author and is rendered
// as text.

import { displayState, type ElementThread, elementSummary, sourceLabel } from "./designComments.ts";
import type { ElementDescriptor, Placement } from "./elementAnchor.ts";
import {
	deleteComment,
	deleteThread,
	editComment,
	replyToThread,
	resolveThread,
} from "./review/api.ts";
import { Composer, ThreadView } from "./review/Thread.tsx";
import "./design.css";
import "./FilesBody.css";

/** One thread in the side list. */
const ElementThreadItem = ({
	n,
	thread,
	state,
	selected,
	busy,
	onSelect,
	onReply,
	onResolve,
	onDelete,
	onEditComment,
	onDeleteComment,
}: {
	n: number;
	thread: ElementThread;
	state: string;
	selected: boolean;
	busy: boolean;
	onSelect: () => void;
	onReply: (body: string) => void;
	onResolve: (resolved: boolean) => void;
	onDelete: () => void;
	onEditComment: (commentId: string, body: string) => void;
	onDeleteComment: (commentId: string) => void;
}) => {
	const a = thread.element?.anchor;
	const src = a ? sourceLabel(a) : undefined;
	return (
		<div className={`dp-comment-item${selected ? " selected" : ""}`} data-state={state}>
			<button type="button" className="dp-comment-head" onClick={onSelect}>
				<span className="dp-comment-n">{n}</span>
				<span className={`dp-comment-state ${state}`}>{state}</span>
				{a && (
					<span className="dp-comment-el" title={elementSummary(a)}>
						{elementSummary(a)}
					</span>
				)}
			</button>
			{state === "lost" && a && (
				<div className="dp-comment-lost">
					element not found
					{src !== undefined && <span className="dp-comment-src"> · was at {src}</span>}
				</div>
			)}
			{state !== "lost" && src !== undefined && <div className="dp-comment-src">{src}</div>}
			<ThreadView
				thread={thread}
				busy={busy}
				hideElementNote
				onReply={onReply}
				onResolve={onResolve}
				onDelete={onDelete}
				onEditComment={onEditComment}
				onDeleteComment={onDeleteComment}
			/>
		</div>
	);
};

export const DesignComments = ({
	roomId,
	entry,
	ready,
	threads,
	placements,
	draft,
	selected,
	busy,
	commentError,
	act,
	onSelect,
	onSubmitDraft,
	onCancelDraft,
}: {
	roomId: string;
	entry: string | undefined;
	ready: boolean;
	threads: ElementThread[];
	placements: Map<string, Placement>;
	draft: ElementDescriptor | null;
	selected: { id: string; tick: number } | null;
	busy: boolean;
	commentError: string | null;
	act: (fn: () => Promise<unknown>) => Promise<void>;
	onSelect: (threadId: string) => void;
	onSubmitDraft: (body: string) => void;
	onCancelDraft: () => void;
}) => (
	<div className="dp-comments">
		{commentError !== null && (
			<div className="fp-notice warn" role="alert">
				{commentError}
			</div>
		)}
		{draft !== null && (
			<div className="dp-comment-draft">
				<div className="dp-comment-el">{elementSummary({ ...draft, entry: entry ?? "" })}</div>
				{sourceLabel(draft) !== undefined && (
					<div className="dp-comment-src">{sourceLabel(draft)}</div>
				)}
				<Composer
					placeholder="Comment on this element…"
					busy={busy}
					submitLabel="Comment"
					autoFocus
					onSubmit={onSubmitDraft}
					onCancel={onCancelDraft}
				/>
			</div>
		)}
		{threads.map((t, i) => (
			<ElementThreadItem
				key={t.id}
				n={i + 1}
				thread={t}
				state={displayState(t, placements, ready)}
				selected={selected?.id === t.id}
				busy={busy}
				onSelect={() => onSelect(t.id)}
				onReply={(body) => void act(() => replyToThread(roomId, t.id, body))}
				onResolve={(r) => void act(() => resolveThread(roomId, t.id, r))}
				onDelete={() => void act(() => deleteThread(roomId, t.id))}
				onEditComment={(id, body) => void act(() => editComment(roomId, id, body))}
				onDeleteComment={(id) => void act(() => deleteComment(roomId, id))}
			/>
		))}
	</div>
);
