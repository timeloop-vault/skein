// The review's file list (#212).
//
// GitHub-shaped, narrowed for a resizable side pane: one row per file
// with its change kind, +/−, comment count and a viewed checkbox. The
// path is split so the filename carries the weight and the directory
// stays legible but quiet — in a 400px column the tail of the path is
// what identifies the file.
//
// "Changes since I last looked" (D1's multi-round loop) rides on the
// viewed marker rather than on a separate mechanism: the backend stores
// the content hash that was viewed, so a file the agent has touched
// since comes back with `changedSinceViewed` and the row says so.

import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";
import type { ReviewFile } from "./api.ts";

const CHANGE_GLYPH: Record<string, string> = {
	added: "A",
	untracked: "A",
	modified: "M",
	deleted: "D",
	renamed: "R",
	conflicted: "!",
	typechange: "T",
};

const splitPath = (path: string): [string, string] => {
	const i = path.lastIndexOf("/");
	return i < 0 ? ["", path] : [path.slice(0, i + 1), path.slice(i + 1)];
};

export const FileList = ({
	files,
	activePath,
	harnessKindOf,
	onSelect,
	onToggleViewed,
}: {
	files: ReviewFile[];
	activePath: string | undefined;
	harnessKindOf: (harnessId: string) => HarnessKind;
	onSelect: (path: string) => void;
	onToggleViewed: (file: ReviewFile) => void;
}) => (
	<div className="rv-files">
		{files.map((f) => {
			const [dir, name] = splitPath(f.path);
			return (
				<div
					key={f.path}
					className={`rv-file${f.path === activePath ? " active" : ""}${f.viewed ? " viewed" : ""}`}
				>
					{/* The checkbox is a sibling of the row button, not a child:
					    nesting an interactive control inside a button makes the
					    click target ambiguous and breaks keyboard traversal. */}
					<input
						type="checkbox"
						className="rv-viewed"
						checked={f.viewed}
						title={
							f.changedSinceViewed
								? "changed since you last looked — tick to mark it seen again"
								: "mark this file as looked at"
						}
						onChange={() => onToggleViewed(f)}
					/>
					<button
						type="button"
						className="rv-file-main"
						onClick={() => onSelect(f.path)}
						title={f.path}
					>
						<span className={`rv-change ${f.change}`}>{CHANGE_GLYPH[f.change] ?? "M"}</span>
						<span className="rv-file-name">
							{dir && <span className="rv-file-dir">{dir}</span>}
							{name}
						</span>
						{f.changedSinceViewed && (
							<span className="rv-since" title="changed since you last looked" />
						)}
						{f.harnessId && <HChip kind={harnessKindOf(f.harnessId)} />}
						{f.hasPending && (
							<span className="rv-pendingdot" title="has uncommitted changes as well" />
						)}
						{f.unresolvedCount > 0 && (
							<span className="rv-badge open" title={`${f.unresolvedCount} unresolved`}>
								{f.unresolvedCount}
							</span>
						)}
						{f.threadCount > f.unresolvedCount && (
							<span className="rv-badge" title={`${f.threadCount} threads in total`}>
								✓
							</span>
						)}
						{f.binary ? (
							<span className="rv-binary">bin</span>
						) : (
							<>
								{f.additions > 0 && <span className="delta-add">+{f.additions}</span>}
								{f.deletions > 0 && <span className="delta-del">−{f.deletions}</span>}
							</>
						)}
					</button>
				</div>
			);
		})}
	</div>
);
