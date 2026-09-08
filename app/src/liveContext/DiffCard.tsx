// Diff card body — issue #211 (epic #52 D3, D4).
//
// The card renders the room's *pending review*: everything since the
// baseline the user last accepted. Before #211 its tabs came from every
// `patch` row the room had ever seen, all-time, so nothing ever cleared
// — not on commit, not on a new harness, not ever. Now a tab clears
// when, and only when, the file is reviewed.
//
// Two verbs, deliberately asymmetric (see crates/skein-review):
//   accept — the baseline moves forward, the worktree is untouched
//   reject — the worktree goes back, the baseline stays put
// Both at hunk and at file granularity, plus accept-all. There is no
// reject-all: it is the destructive one, and it wants choosing per file.
//
// Harness is attribution, not scope (D4). All harnesses in a room share
// one worktree, so there is exactly one diff; which harness wrote a hunk
// is a chip and a filter, never a partition.

import type { ReactNode } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";
import "./diff.css";
import {
	type BlockedReason,
	type PendingFile,
	type ReviewHunk,
	acceptReview,
	attributeHunks,
	contributingHarnesses,
	rejectReview,
} from "./review.ts";
import type { HarnessAction } from "./store.ts";
import { useReviewPending } from "./useReviewPending.ts";

const BLOCKED_TEXT: Record<BlockedReason, string> = {
	binary: "binary file",
	toolarge: "file is too large to diff",
	symlink: "symlink",
	unreadable: "file could not be read",
};

const lineClass = (kind: ReviewHunk["lines"][number]["kind"]) =>
	kind === "add" ? "lc-line add" : kind === "delete" ? "lc-line del" : "lc-line";

const DiffEmpty = ({ glyph, text }: { glyph: string; text: string }) => (
	<div className="lc-empty">
		<div className="lc-empty-inner">
			<div className="big">{glyph}</div>
			{text}
		</div>
	</div>
);

/// The accept / reject pair. `busy` disables both while a call is in
/// flight so a double-click can't submit the same hunk twice.
const Verbs = ({
	busy,
	onAccept,
	onReject,
	acceptTitle,
	rejectTitle,
}: {
	busy: boolean;
	onAccept: () => void;
	onReject: () => void;
	acceptTitle: string;
	rejectTitle: string;
}) => (
	<span className="lc-diff-verbs">
		<button
			type="button"
			className="lc-diff-act accept"
			disabled={busy}
			title={acceptTitle}
			onClick={onAccept}
		>
			✓
		</button>
		<button
			type="button"
			className="lc-diff-act reject"
			disabled={busy}
			title={rejectTitle}
			onClick={onReject}
		>
			↶
		</button>
	</span>
);

const Hunks = ({
	file,
	owners,
	harnessKindOf,
	busy,
	onAccept,
	onReject,
}: {
	file: PendingFile;
	owners: Array<string | undefined>;
	harnessKindOf: (harnessId: string) => HarnessKind;
	busy: boolean;
	onAccept: (h: ReviewHunk) => void;
	onReject: (h: ReviewHunk) => void;
}) => (
	<>
		{file.hunks.map((h, hi) => {
			const owner = owners[hi];
			return (
				<div key={`${h.oldStart}-${h.newStart}-${hi}`}>
					<div className="lc-diff-hunk-sep">
						{owner && <HChip kind={harnessKindOf(owner)} />}
						<span className="lc-diff-hunk-range">{h.header}</span>
						<Verbs
							busy={busy}
							onAccept={() => onAccept(h)}
							onReject={() => onReject(h)}
							acceptTitle="accept this hunk — advances the baseline, leaves the file as it is"
							rejectTitle="reject this hunk — restores the baseline content on disk"
						/>
					</div>
					{h.lines.map((l, li) => (
						<div key={`${hi}-${li}`} className={lineClass(l.kind)}>
							<span className="gutter">
								<span className="ln">{l.oldLineno ?? ""}</span>
								<span className="ln">{l.newLineno ?? ""}</span>
							</span>
							<span className="marker">
								{l.kind === "add" ? "+" : l.kind === "delete" ? "−" : ""}
							</span>
							<span className="src">{l.content}</span>
						</div>
					))}
				</div>
			);
		})}
	</>
);

export const DiffCardBody = ({
	roomId,
	cwd,
	actions,
	harnessKindOf,
	focusedHarnessId,
	visible,
}: {
	roomId: string;
	cwd: string;
	/** The room's action rows — used only for per-hunk attribution. */
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	/** The harness the user is chatting with — the card auto-focuses its
	 *  latest pending file. */
	focusedHarnessId: string | undefined;
	/** Only the visible room runs the watcher (see useReviewPending). */
	visible: boolean;
}) => {
	const { files, error, refresh } = useReviewPending(roomId, cwd, visible);
	const [busy, setBusy] = useState(false);
	const [actionError, setActionError] = useState<string | undefined>(undefined);
	// Harness filter (D4). A filter over one diff, never a partition of it.
	const [filter, setFilter] = useState<string | undefined>(undefined);

	const harnesses = useMemo(() => contributingHarnesses(files), [files]);
	const shown = useMemo(
		() => (filter ? files.filter((f) => f.harnessId === filter) : files),
		[files, filter],
	);

	// A filter whose harness no longer owns anything pending would hide
	// the whole card; drop it rather than show an empty pane.
	useEffect(() => {
		if (filter && !harnesses.includes(filter)) setFilter(undefined);
	}, [filter, harnesses]);

	// Auto-focus: the focused harness's most recent pending file, else
	// the most recent by anyone (files arrive oldest-touch-first).
	const auto = useMemo(() => {
		const last = (list: PendingFile[]) => list[list.length - 1];
		const mine = focusedHarnessId ? shown.filter((f) => f.harnessId === focusedHarnessId) : [];
		return (last(mine) ?? last(shown))?.path;
	}, [shown, focusedHarnessId]);

	// Manual tab selection wins until auto-focus moves to another file.
	const [manual, setManual] = useState<string | undefined>(undefined);
	const prevAuto = useRef(auto);
	useEffect(() => {
		if (prevAuto.current !== auto) {
			prevAuto.current = auto;
			setManual(undefined);
		}
	}, [auto]);
	const activePath = shown.some((f) => f.path === manual) ? manual : auto;
	const active = shown.find((f) => f.path === activePath);

	const owners = useMemo(() => (active ? attributeHunks(actions, active) : []), [actions, active]);

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
				// reason a call fails is that the file moved, and the
				// user needs to see what it moved to.
				refresh();
			});
	};

	const acceptFile = (f: PendingFile) =>
		run(() => acceptReview(roomId, cwd, f.path, [], f.contentHash));
	const rejectFile = (f: PendingFile) =>
		run(() => rejectReview(roomId, cwd, f.path, [], f.contentHash));
	const acceptAll = () => run(() => acceptReview(roomId, cwd, undefined, [], undefined));

	if (files.length === 0) {
		return (
			<DiffEmpty
				glyph="◇"
				text={
					error
						? `could not read the review: ${error}`
						: "when an agent edits a file, its diff appears here"
				}
			/>
		);
	}

	let body: ReactNode;
	if (!active) {
		body = <DiffEmpty glyph="◇" text="select a file" />;
	} else if (active.blocked) {
		body = <DiffEmpty glyph="◇" text={BLOCKED_TEXT[active.blocked]} />;
	} else {
		body = (
			<Hunks
				file={active}
				owners={owners}
				harnessKindOf={harnessKindOf}
				busy={busy}
				onAccept={(h) => run(() => acceptReview(roomId, cwd, active.path, [h], undefined))}
				onReject={(h) => run(() => rejectReview(roomId, cwd, active.path, [h], undefined))}
			/>
		);
	}

	return (
		<div className="lc-diff">
			<div className="lc-diff-tabs">
				{shown.map((t) => {
					const isActive = t.path === activePath;
					// Flicker: another harness owns this file and you're not
					// looking at it (handover §5.1).
					const flicker =
						!isActive && focusedHarnessId !== undefined && t.harnessId !== focusedHarnessId;
					return (
						<button
							type="button"
							key={t.path}
							className={`lc-diff-tab${isActive ? " active" : ""}${flicker ? " flicker" : ""}`}
							title={`${t.path} (${t.change})`}
							onClick={() => setManual(t.path)}
						>
							<HChip kind={harnessKindOf(t.harnessId)} />
							<span>{t.name}</span>
							{t.additions > 0 && <span className="delta-add">+{t.additions}</span>}
							{t.deletions > 0 && <span className="delta-del">−{t.deletions}</span>}
						</button>
					);
				})}
			</div>

			<div className="lc-diff-toolbar">
				{harnesses.length > 1 && (
					<span className="lc-diff-filter">
						<button
							type="button"
							className={`lc-diff-chip${filter === undefined ? " on" : ""}`}
							onClick={() => setFilter(undefined)}
							title="show every harness's changes"
						>
							all
						</button>
						{harnesses.map((h) => (
							<button
								type="button"
								key={h}
								className={`lc-diff-chip${filter === h ? " on" : ""}`}
								onClick={() => setFilter(filter === h ? undefined : h)}
								title={`only files last written by ${harnessKindOf(h)}`}
							>
								<HChip kind={harnessKindOf(h)} />
							</button>
						))}
					</span>
				)}
				<span className="lc-diff-toolbar-right">
					{active && (
						<Verbs
							busy={busy}
							onAccept={() => acceptFile(active)}
							onReject={() => rejectFile(active)}
							acceptTitle={`accept all of ${active.name} — advances the baseline, leaves the file as it is`}
							rejectTitle={`reject all of ${active.name} — restores the baseline content on disk`}
						/>
					)}
					<button
						type="button"
						className="lc-diff-act all"
						disabled={busy}
						onClick={acceptAll}
						title="accept every pending file — advances the baseline, writes nothing"
					>
						✓ all
					</button>
				</span>
			</div>

			{(actionError ?? error) && <div className="lc-diff-err">{actionError ?? error}</div>}

			{/* Keyed by the active file so a focus jump remounts the body —
			    replays the refocus glow and resets scroll to the top. */}
			<div className="lc-diff-body refocus" key={activePath ?? "none"}>
				{body}
			</div>
		</div>
	);
};
