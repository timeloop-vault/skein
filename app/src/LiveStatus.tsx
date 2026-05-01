// LiveStatus — real "what's changed?" pane for sessions with a real cwd.
//
// Phase 5a: pulls a snapshot via `git_status` on mount and on demand.
// Phase 5b will subscribe to a file-watcher event and refresh in
// response — until then this is poll-only.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";

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
	const [error, setError] = useState<string | null>(null);
	const [loading, setLoading] = useState(false);

	const refresh = useCallback(async () => {
		setLoading(true);
		setError(null);
		try {
			const rows = await invoke<StatusDto[]>("git_status", { path: cwd });
			setEntries(rows);
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
					flex: 1,
					overflowY: "auto",
					padding: "6px 0",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
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
					return (
						<div
							key={`${e.path}:${e.staged}:${i}`}
							style={{
								display: "flex",
								gap: 10,
								padding: "3px 14px",
								color: e.staged ? "var(--fg-0)" : "var(--fg-1)",
							}}
							title={`${e.staged ? "staged " : ""}${meta.label}`}
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
		</div>
	);
};
