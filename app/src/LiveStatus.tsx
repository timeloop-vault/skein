// LiveStatus — real "what's changed?" pane for sessions with a real cwd.
//
// Phase 5b: on mount we fetch the initial snapshot AND start a debounced
// filesystem watcher; each tick re-fetches.
// Phase 5c: clicking a row reveals its diff in the bottom half. Diff is
// re-fetched on every refresh so it stays in sync with the file list.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";

type StatusKind =
	| "added"
	| "modified"
	| "deleted"
	| "renamed"
	| "untracked"
	| "conflicted"
	| "typechange";

interface StatusDto {
	path: string;
	kind: StatusKind;
	staged: boolean;
}

type DiffLineKind = "context" | "add" | "delete";

interface DiffLineDto {
	kind: DiffLineKind;
	content: string;
	oldLineno?: number;
	newLineno?: number;
}

interface DiffHunkDto {
	header: string;
	lines: DiffLineDto[];
}

interface FileDiffDto {
	path: string;
	kind: StatusKind;
	binary: boolean;
	hunks: DiffHunkDto[];
}

const KIND_GLYPH: Record<StatusKind, { glyph: string; color: string; label: string }> = {
	added: { glyph: "A", color: "var(--ok)", label: "added" },
	modified: { glyph: "M", color: "var(--warn)", label: "modified" },
	deleted: { glyph: "D", color: "var(--err)", label: "deleted" },
	renamed: { glyph: "R", color: "var(--waiting)", label: "renamed" },
	untracked: { glyph: "?", color: "var(--fg-2)", label: "untracked" },
	conflicted: { glyph: "!", color: "var(--err)", label: "conflicted" },
	typechange: { glyph: "T", color: "var(--waiting)", label: "type changed" },
};

interface LiveStatusProps {
	cwd: string;
}

export const LiveStatus = ({ cwd }: LiveStatusProps) => {
	const [entries, setEntries] = useState<StatusDto[] | null>(null);
	const [diffs, setDiffs] = useState<FileDiffDto[]>([]);
	const [selectedPath, setSelectedPath] = useState<string | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [loading, setLoading] = useState(false);
	// Header dot pulses for 600ms after every watcher-driven refresh so
	// the user can *see* the auto-update happening — manual refresh /
	// initial mount don't pulse.
	const [pulse, setPulse] = useState(false);
	const fromWatcherRef = useRef(false);

	const refresh = useCallback(async () => {
		setLoading(true);
		setError(null);
		try {
			// Status and diff are independent calls — fire in parallel
			// so a diff containing a few large files doesn't hold up
			// the file list rendering.
			const [rows, files] = await Promise.all([
				invoke<StatusDto[]>("git_status", { path: cwd }),
				invoke<FileDiffDto[]>("git_diff", { path: cwd }),
			]);
			setEntries(rows);
			setDiffs(files);
			if (fromWatcherRef.current) {
				setPulse(true);
				window.setTimeout(() => setPulse(false), 600);
				fromWatcherRef.current = false;
			}
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			setError(msg);
		} finally {
			setLoading(false);
		}
	}, [cwd]);

	useEffect(() => {
		void refresh();
	}, [refresh]);

	// File watcher — fires `refresh` whenever the worktree changes
	// (debounced 200ms on the Rust side).
	useEffect(() => {
		const channel = new Channel<null>();
		channel.onmessage = () => {
			fromWatcherRef.current = true;
			void refresh();
		};

		let watchId: string | null = null;
		let cancelled = false;

		invoke<string>("git_watch_start", { path: cwd, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] git_watch_start failed:", msg);
			});

		return () => {
			cancelled = true;
			if (watchId) {
				void invoke("git_watch_stop", { id: watchId });
			}
		};
	}, [cwd, refresh]);

	// If the selected file disappears from the diff (e.g. user reverted
	// it, deleted it from the worktree), clear the selection so the
	// diff pane doesn't cling to stale state.
	useEffect(() => {
		if (selectedPath && !diffs.some((f) => f.path === selectedPath)) {
			setSelectedPath(null);
		}
	}, [diffs, selectedPath]);

	const selectedDiff = selectedPath ? (diffs.find((f) => f.path === selectedPath) ?? null) : null;

	return (
		<div
			style={{
				flex: 1,
				display: "flex",
				flexDirection: "column",
				minHeight: 0,
				background: "var(--bg-1)",
			}}
		>
			<div
				style={{
					padding: "10px 14px",
					borderBottom: "1px solid var(--line)",
					display: "flex",
					alignItems: "center",
					gap: 10,
					background: "var(--bg-0)",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					color: "var(--fg-2)",
				}}
			>
				<span style={{ color: "var(--fg-0)" }}>Status</span>
				<span
					style={{
						width: 6,
						height: 6,
						borderRadius: "50%",
						background: pulse ? "var(--accent)" : "var(--ok)",
						boxShadow: `0 0 ${pulse ? 8 : 4}px ${pulse ? "var(--accent)" : "var(--ok)"}`,
						transition: "background 0.2s, box-shadow 0.2s",
					}}
					title={pulse ? "just refreshed" : "watching"}
				/>
				<span style={{ color: "var(--fg-3)" }}>·</span>
				<span style={{ color: "var(--fg-3)", flex: 1, minWidth: 0 }} title={cwd}>
					<span
						style={{
							display: "inline-block",
							maxWidth: "100%",
							overflow: "hidden",
							textOverflow: "ellipsis",
							whiteSpace: "nowrap",
							verticalAlign: "bottom",
						}}
					>
						{cwd}
					</span>
				</span>
				<button className="sk-btn ghost" onClick={() => void refresh()} disabled={loading}>
					{loading ? "…" : "Refresh"}
				</button>
			</div>

			{error && (
				<div
					style={{
						margin: "10px 14px",
						padding: "8px 10px",
						color: "var(--err)",
						fontFamily: "var(--sk-mono)",
						fontSize: 11,
						background: "color-mix(in srgb, var(--err) 8%, var(--bg-2))",
						border: "1px solid color-mix(in srgb, var(--err) 35%, var(--line))",
						borderRadius: 5,
					}}
				>
					{error}
				</div>
			)}

			<div
				style={{
					// Cap the file list at ~38% of the pane height when a
					// file is selected so the diff gets the lion's share.
					// Without selection, the list expands to fill the pane.
					flex: selectedDiff ? "0 0 38%" : 1,
					overflowY: "auto",
					padding: "6px 0",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					borderBottom: selectedDiff ? "1px solid var(--line)" : "none",
				}}
			>
				{entries === null && !error && (
					<div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>loading…</div>
				)}
				{entries !== null && entries.length === 0 && (
					<div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>
						worktree clean — nothing to show
					</div>
				)}
				{entries?.map((e, i) => {
					const meta = KIND_GLYPH[e.kind];
					const selected = e.path === selectedPath;
					const hasDiff = diffs.some((f) => f.path === e.path);
					return (
						<div
							key={`${e.path}:${e.staged}:${i}`}
							onClick={() => hasDiff && setSelectedPath(selected ? null : e.path)}
							style={{
								display: "flex",
								gap: 10,
								padding: "3px 14px",
								color: e.staged ? "var(--fg-0)" : "var(--fg-1)",
								cursor: hasDiff ? "pointer" : "default",
								background: selected ? "var(--bg-3)" : "transparent",
							}}
							title={`${e.staged ? "staged " : ""}${meta.label}${
								hasDiff ? "" : " (no diff available)"
							}`}
						>
							<span
								style={{
									color: meta.color,
									width: 14,
									textAlign: "center",
									fontWeight: 600,
								}}
							>
								{meta.glyph}
							</span>
							<span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>
								{e.path}
							</span>
							{e.staged && <span style={{ color: "var(--fg-3)", fontSize: 10 }}>staged</span>}
						</div>
					);
				})}
			</div>

			{selectedDiff && <DiffView file={selectedDiff} onClose={() => setSelectedPath(null)} />}
		</div>
	);
};

// ── DiffView ───────────────────────────────────────────────────────
// Compact patch renderer: hunk headers in dim mono, gutter with old/new
// line numbers, +/- coloring on the line content. Reuses the .sk-line /
// .sk-code classes from the design's CSS so the visual language matches
// the right pane's other diff variants.

const DiffView = ({ file, onClose }: { file: FileDiffDto; onClose: () => void }) => {
	const meta = KIND_GLYPH[file.kind];
	return (
		<div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
			<div
				style={{
					padding: "8px 14px",
					borderBottom: "1px solid var(--line)",
					display: "flex",
					alignItems: "center",
					gap: 10,
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					color: "var(--fg-2)",
					background: "var(--bg-0)",
				}}
			>
				<span style={{ color: meta.color, fontWeight: 600 }}>{meta.glyph}</span>
				<span
					style={{
						color: "var(--fg-0)",
						flex: 1,
						minWidth: 0,
						overflow: "hidden",
						textOverflow: "ellipsis",
					}}
				>
					{file.path}
				</span>
				<button className="sk-btn ghost" onClick={onClose} title="Close diff">
					×
				</button>
			</div>
			<div className="sk-code" style={{ flex: 1, padding: "6px 0" }}>
				{file.binary && (
					<div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>
						binary file changed — no patch to show
					</div>
				)}
				{!file.binary && file.hunks.length === 0 && (
					<div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>
						no hunks — file metadata changed only
					</div>
				)}
				{file.hunks.map((hunk, hi) => (
					<DiffHunkBlock key={`${file.path}:${hi}`} hunk={hunk} />
				))}
			</div>
		</div>
	);
};

const DiffHunkBlock = ({ hunk }: { hunk: DiffHunkDto }) => (
	<>
		<div
			style={{
				padding: "2px 12px",
				color: "var(--fg-3)",
				fontFamily: "var(--sk-mono)",
				fontSize: 10.5,
				background: "var(--bg-0)",
				borderTop: "1px solid var(--line)",
				borderBottom: "1px solid var(--line)",
			}}
		>
			{hunk.header}
		</div>
		{hunk.lines.map((line, li) => (
			<div
				key={li}
				className={`sk-line ${line.kind === "add" ? "add" : line.kind === "delete" ? "del" : ""}`}
			>
				<div className="gutter">
					<span className="ln">{line.oldLineno ?? ""}</span>
					<span className="ln">{line.newLineno ?? ""}</span>
				</div>
				<span className="marker">
					{line.kind === "add" ? "+" : line.kind === "delete" ? "−" : ""}
				</span>
				<span className="src">{line.content}</span>
			</div>
		))}
	</>
);
