// LiveStatus — real "what's changed?" pane for sessions with a real cwd.
//
// Phase 5b: on mount we both fetch the initial snapshot AND start a
// debounced filesystem watcher. Each watcher tick fires a re-fetch, so
// the list updates ~250ms after a harness writes to a file.

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
	// "Just refreshed" badge for the header — flashes for 600ms after
	// every watcher-driven refresh so the user can see the auto-update
	// is real, not just an empty UI.
	const [pulse, setPulse] = useState(false);
	// Track whether the *current* refresh was triggered by a watcher
	// event vs. the initial mount or manual click. Only watcher-driven
	// refreshes pulse — manual ones don't need the visual feedback.
	const fromWatcherRef = useRef(false);

	const refresh = useCallback(async () => {
		setLoading(true);
		setError(null);
		try {
			const rows = await invoke<StatusDto[]>("git_status", { path: cwd });
			setEntries(rows);
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
