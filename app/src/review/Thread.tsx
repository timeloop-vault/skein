// Comment threads and the composer (#212, epic #52 D5/D6).
//
// D5: flat comments with replies. There is no nesting here because
// there is none in the model — a reply is another row on the same
// thread, so the component is a header, a list, and a reply box.
//
// D6 shows up in the header. A thread whose anchor could not be
// re-matched exactly says so and renders the code it was written
// against, because the alternative — quietly sitting on whatever now
// occupies those line numbers — is the failure the whole anchoring
// model exists to prevent.

import { useEffect, useRef, useState } from "react";
import type { ReviewComment, ReviewThread } from "./api.ts";
import "./Thread.css";
import "./element.css";

/// Short relative time — the review reads in minutes and hours, and a
/// full timestamp in a narrow pane costs more than it tells.
export function ago(ms: number): string {
	const secs = Math.max(0, Math.floor((Date.now() - ms) / 1000));
	if (secs < 60) return "just now";
	const mins = Math.floor(secs / 60);
	if (mins < 60) return `${mins}m ago`;
	const hours = Math.floor(mins / 60);
	if (hours < 24) return `${hours}h ago`;
	const days = Math.floor(hours / 24);
	if (days < 7) return `${days}d ago`;
	return new Date(ms).toLocaleDateString();
}

/// A textarea that submits on Mod+Enter and cancels on Escape.
///
/// Mod+Enter rather than Enter: review comments run to several lines
/// often enough that a submitting Enter would eat half of them.
export const Composer = ({
	placeholder,
	initial,
	busy,
	submitLabel,
	autoFocus,
	onSubmit,
	onCancel,
}: {
	placeholder: string;
	initial?: string;
	busy: boolean;
	submitLabel: string;
	autoFocus?: boolean;
	onSubmit: (body: string) => void;
	onCancel?: () => void;
}) => {
	const [text, setText] = useState(initial ?? "");
	const ref = useRef<HTMLTextAreaElement>(null);

	useEffect(() => {
		if (autoFocus) ref.current?.focus();
	}, [autoFocus]);

	const submit = () => {
		const body = text.trim();
		if (!body || busy) return;
		onSubmit(body);
		setText("");
	};

	return (
		<div className="rv-composer">
			<textarea
				ref={ref}
				className="rv-composer-input"
				placeholder={placeholder}
				value={text}
				disabled={busy}
				rows={3}
				onChange={(e) => setText(e.target.value)}
				onKeyDown={(e) => {
					// Stop every key reaching App's window-level shortcut
					// dispatcher — Alt is the app modifier on Windows, and
					// AltGr on a Swedish layout is Ctrl+Alt, so typing a
					// brace mid-comment would otherwise fire a shortcut.
					e.stopPropagation();
					if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
						e.preventDefault();
						submit();
					} else if (e.key === "Escape" && onCancel) {
						e.preventDefault();
						onCancel();
					}
				}}
			/>
			<div className="rv-composer-actions">
				<span className="rv-hint">⌘/Ctrl ↵</span>
				{onCancel && (
					<button type="button" className="rv-btn" onClick={onCancel} disabled={busy}>
						cancel
					</button>
				)}
				<button
					type="button"
					className="rv-btn primary"
					onClick={submit}
					disabled={busy || text.trim() === ""}
				>
					{submitLabel}
				</button>
			</div>
		</div>
	);
};

const CommentBody = ({
	comment,
	busy,
	onEdit,
	onDelete,
}: {
	comment: ReviewComment;
	busy: boolean;
	onEdit: (body: string) => void;
	onDelete: () => void;
}) => {
	const [editing, setEditing] = useState(false);
	const isAgent = comment.authorKind === "agent";

	if (editing) {
		return (
			<Composer
				placeholder="edit this comment"
				initial={comment.body}
				busy={busy}
				submitLabel="save"
				autoFocus
				onSubmit={(body) => {
					onEdit(body);
					setEditing(false);
				}}
				onCancel={() => setEditing(false)}
			/>
		);
	}

	return (
		<div className="rv-comment">
			<div className="rv-comment-head">
				<span className={`rv-author${isAgent ? " agent" : ""}`}>
					{isAgent ? (comment.authorLabel ?? "agent") : "you"}
				</span>
				<span className="rv-time">{ago(comment.createdMs)}</span>
				{comment.updatedMs > comment.createdMs && <span className="rv-time">· edited</span>}
				<span className="rv-comment-actions">
					<button
						type="button"
						className="rv-linkbtn"
						onClick={() => setEditing(true)}
						disabled={busy}
					>
						edit
					</button>
					<button type="button" className="rv-linkbtn" onClick={onDelete} disabled={busy}>
						delete
					</button>
				</span>
			</div>
			<div className="rv-comment-body">{comment.body}</div>
		</div>
	);
};

/// The banner an inexactly-anchored thread wears.
///
/// `moved` gets none: a verbatim match elsewhere in the file is a
/// certainty, not a guess, and badging it would train the user to
/// ignore the badge that matters.
const PlacementNote = ({ thread }: { thread: ReviewThread }) => {
	if (!thread.outdated || thread.scope === "element") return null;
	const pct = thread.confidence == null ? null : Math.round(thread.confidence * 100);
	return (
		<div className="rv-outdated">
			<span className="rv-outdated-tag">
				{thread.placement === "shifted" ? "moved?" : "outdated"}
			</span>
			<span>
				{thread.placement === "shifted"
					? `the code under this comment changed — ${pct ?? "?"}% of it still matches`
					: "the code this was written about is gone"}
			</span>
			{thread.anchorLines.length > 0 && (
				<pre className="rv-anchor">{thread.anchorLines.join("\n")}</pre>
			)}
		</div>
	);
};

/// Shorten untrusted page text for display; the full value stays in the
/// tooltip. Plain strings only — they are rendered as React text.
const clip = (s: string, max: number): string => (s.length > max ? `${s.slice(0, max)}…` : s);

const ELEMENT_MISS: Record<string, string> = {
	stale: "element changed — showing original evidence",
	lost: "element not found — showing original evidence",
	unknown: "element not checked yet — showing original evidence",
};

/// The anchor summary an element thread (#434) wears: what was commented
/// on, where it came from, and whether the design pane can still find it.
/// Every value is page content and therefore untrusted.
const ElementNote = ({
	thread,
	design,
}: {
	thread: ReviewThread;
	design: DesignLink | undefined;
}) => {
	const el = thread.element;
	if (!el) return null;
	const { anchor, state } = el;
	const found = state === "anchored" || state === "reanchored";
	const src = anchor.source ? `${anchor.source.file}:${anchor.source.line}` : undefined;
	const label = anchor.odId ?? anchor.selector;
	return (
		<div className={`rv-element${found ? "" : " rv-outdated"}`}>
			<div className="rv-element-head">
				<span className="rv-outdated-tag" title={`element ${state}`}>
					{state}
				</span>
				<code title={anchor.selector}>{`<${clip(anchor.tag, 24)}>`}</code>
				<span title={label}>{clip(label, 48)}</span>
				{src && <span title={src}>{clip(src, 60)}</span>}
			</div>
			{anchor.text !== "" && <div title={anchor.text}>{clip(anchor.text, 120)}</div>}
			{!found && <span>{ELEMENT_MISS[state] ?? ELEMENT_MISS.unknown}</span>}
			{design && (
				<button
					type="button"
					className="rv-linkbtn"
					disabled={!design.available}
					title={design.available ? "focus this element in the design pane" : NO_DESIGN_TITLE}
					onClick={design.onShow}
				>
					show in design pane
				</button>
			)}
		</div>
	);
};

const NO_DESIGN_TITLE = "Add a design harness to this room to see it";

/// How an element thread reaches the room's design harness. Absent when
/// the surface rendering the thread has no design pane to offer.
export type DesignLink = { available: boolean; onShow: () => void };

/// The agent's "I handled this" claim (#213).
///
/// Rendered above the conversation and *beside* the resolve control,
/// never instead of it: the whole point of D8 withholding resolve from
/// the agent is that a claim still has to be read before the thread
/// closes.
const AddressedNote = ({ thread }: { thread: ReviewThread }) => {
	const a = thread.addressed;
	if (!a) return null;
	return (
		<div className="rv-addressed">
			<span className="rv-addressed-tag">addressed</span>
			<span>
				{a.by}
				{a.commitSha ? ` · ${a.commitSha.slice(0, 7)}` : ""}
				{a.note ? ` · ${a.note}` : ""}
			</span>
		</div>
	);
};

export const ThreadView = ({
	thread,
	busy,
	onReply,
	onResolve,
	onDelete,
	onEditComment,
	onDeleteComment,
	design,
	hideElementNote = false,
}: {
	thread: ReviewThread;
	busy: boolean;
	design?: DesignLink | undefined;
	/// The design pane prints its own element header, so it asks for it once.
	hideElementNote?: boolean;
	onReply: (body: string) => void;
	onResolve: (resolved: boolean) => void;
	onDelete: () => void;
	onEditComment: (commentId: string, body: string) => void;
	onDeleteComment: (commentId: string) => void;
}) => {
	const resolved = thread.resolvedMs != null;
	// A resolved thread collapses to its head: the review is about what
	// is still open, and a wall of settled threads buries it.
	const [open, setOpen] = useState(!resolved);
	const [replying, setReplying] = useState(false);

	return (
		<div className={`rv-thread${resolved ? " resolved" : ""}${thread.outdated ? " outdated" : ""}`}>
			<div className="rv-thread-head">
				<button type="button" className="rv-chev" onClick={() => setOpen(!open)}>
					{open ? "▾" : "▸"}
				</button>
				<span className="rv-thread-title">
					{resolved ? "resolved" : "open"}
					{thread.comments.length > 1 && ` · ${thread.comments.length} comments`}
				</span>
				<span className="rv-thread-actions">
					<button
						type="button"
						className="rv-linkbtn"
						onClick={() => onResolve(!resolved)}
						disabled={busy}
						title={
							resolved
								? "reopen this thread"
								: "mark this thread resolved — only you can, never the agent (D8)"
						}
					>
						{resolved ? "reopen" : "resolve"}
					</button>
					<button type="button" className="rv-linkbtn" onClick={onDelete} disabled={busy}>
						delete thread
					</button>
				</span>
			</div>

			{open && (
				<>
					<PlacementNote thread={thread} />
					{!hideElementNote && <ElementNote thread={thread} design={design} />}
					<AddressedNote thread={thread} />
					{thread.comments.map((c) => (
						<CommentBody
							key={c.id}
							comment={c}
							busy={busy}
							onEdit={(body) => onEditComment(c.id, body)}
							onDelete={() => onDeleteComment(c.id)}
						/>
					))}
					{replying ? (
						<Composer
							placeholder="reply"
							busy={busy}
							submitLabel="reply"
							autoFocus
							onSubmit={(body) => {
								onReply(body);
								setReplying(false);
							}}
							onCancel={() => setReplying(false)}
						/>
					) : (
						<button
							type="button"
							className="rv-linkbtn rv-reply-open"
							onClick={() => setReplying(true)}
							disabled={busy}
						>
							reply
						</button>
					)}
				</>
			)}
		</div>
	);
};
