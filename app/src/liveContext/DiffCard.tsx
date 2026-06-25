// Diff card body — issue #80 D3.
//
// Harness-first like the other cards: the tabs, the active file, and the
// per-tab flicker all derive from the room's patch rows (what the agents
// touched). The body prefers the live worktree diff (git_diff) and falls
// back to the harness's own reported patch when git can't show the file
// (no repo, gitignored, untracked-in-non-repo) — so it's never blank for
// something an agent demonstrably edited. Spec: handover §5.1. Sticky /
// jump-to-latest (§5.1) is deferred per the handover; auto-focus re-takes
// the view when the agent moves to a different file.

import type { ReactNode } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";
import { orderForDisplay } from "./ActivityCard.tsx";
import "./diff.css";
import {
	type DiffHunk,
	autoFocusFile,
	deriveTabs,
	harnessPatchHunks,
	matchGitFile,
} from "./diff.ts";
import type { HarnessAction } from "./store.ts";
import { useWorktreeDiff } from "./useWorktreeDiff.ts";

const lineClass = (kind: DiffHunk["lines"][number]["kind"]) =>
	kind === "add" ? "lc-line add" : kind === "delete" ? "lc-line del" : "lc-line";

const DiffLines = ({ hunks }: { hunks: DiffHunk[] }) => (
	<>
		{hunks.map((h, hi) => (
			<div key={`${h.header}-${hi}`}>
				{hi > 0 && <div className="lc-diff-hunk-sep">{h.header}</div>}
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
		))}
	</>
);

const DiffEmpty = ({ glyph, text }: { glyph: string; text: string }) => (
	<div className="lc-empty">
		<div className="lc-empty-inner">
			<div className="big">{glyph}</div>
			{text}
		</div>
	</div>
);

export const DiffCardBody = ({
	cwd,
	actions,
	harnessKindOf,
	focusedHarnessId,
	visible,
}: {
	cwd: string;
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	/** The harness the user is chatting with — the card auto-focuses its
	 *  latest edit. */
	focusedHarnessId: string | undefined;
	/** Only the visible room runs the git_diff watcher (see useWorktreeDiff). */
	visible: boolean;
}) => {
	const gitFiles = useWorktreeDiff(cwd, visible);
	// Chronological order (carry-forward for ts=0 rows) so "latest edit"
	// is correct even for timestamp-less Claude patch rows — same ordering
	// the Activity feed uses.
	const ordered = useMemo(() => orderForDisplay(actions), [actions]);
	const tabs = useMemo(() => deriveTabs(ordered), [ordered]);
	const auto = useMemo(() => autoFocusFile(ordered, focusedHarnessId), [ordered, focusedHarnessId]);

	// Manual tab selection overrides auto-focus until the agent moves to a
	// different file (no sticky timer in v1 — auto-focus then re-takes it).
	const [manual, setManual] = useState<string | undefined>(undefined);
	const prevAuto = useRef(auto);
	useEffect(() => {
		if (prevAuto.current !== auto) {
			prevAuto.current = auto;
			setManual(undefined);
		}
	}, [auto]);
	const activeFile = manual ?? auto;

	if (tabs.length === 0) {
		return <DiffEmpty glyph="◇" text="when an agent edits a file, its diff appears here" />;
	}

	// Body for the active file: git worktree diff first, harness patch as
	// the fallback that covers non-git / ignored / untracked paths.
	let body: ReactNode;
	if (!activeFile) {
		body = <DiffEmpty glyph="◇" text="select a file" />;
	} else {
		const gitFile = matchGitFile(gitFiles, activeFile);
		if (gitFile?.binary) {
			body = <DiffEmpty glyph="◇" text="binary file" />;
		} else if (gitFile && gitFile.hunks.length > 0) {
			body = <DiffLines hunks={gitFile.hunks} />;
		} else {
			const fallback = harnessPatchHunks(ordered, activeFile);
			body =
				fallback && fallback.length > 0 ? (
					<DiffLines hunks={fallback} />
				) : (
					<DiffEmpty glyph="◇" text="no changes in the worktree" />
				);
		}
	}

	return (
		<div className="lc-diff">
			<div className="lc-diff-tabs">
				{tabs.map((t) => {
					const active = t.fullPath === activeFile;
					// Flicker: another harness owns the latest edit here and
					// you're not looking at it (handover §5.1). Needs a known
					// focused harness — otherwise every non-active tab would.
					const flicker =
						!active && focusedHarnessId !== undefined && t.harnessId !== focusedHarnessId;
					return (
						<button
							type="button"
							key={t.fullPath}
							className={`lc-diff-tab${active ? " active" : ""}${flicker ? " flicker" : ""}`}
							title={t.fullPath}
							onClick={() => setManual(t.fullPath)}
						>
							<HChip kind={harnessKindOf(t.harnessId)} />
							<span>{t.file}</span>
							{t.adds != null && <span className="delta-add">+{t.adds}</span>}
							{t.dels != null && <span className="delta-del">−{t.dels}</span>}
						</button>
					);
				})}
			</div>
			{/* Keyed by the active file so a focus jump remounts the body —
			    replays the refocus glow and resets scroll to the top. */}
			<div className="lc-diff-body refocus" key={activeFile ?? "none"}>
				{body}
			</div>
		</div>
	);
};
