// FileTree — the browse+view surface of a `files` harness body (#49;
// originally issue #7, deleted in the Live Context rework,
// resurrected from e8ecbab^ as an overlay in stage 1, then moved
// into the harness body slot in phase A).
//
// One-level directory listing rooted at the room's cwd, navigable
// in-place: click a dir to descend, click a breadcrumb segment to go
// back up, click a file to see its raw text on the right. Sorted by
// mtime descending so the things the agent just wrote bubble to the
// top.
//
// The right side is deliberately a RAW view (no rendered markdown,
// no image/hex providers — stripped 2026-07-13 per daily-driver
// feedback): the want is VS Code-style raw view/edit, and stage 2
// replaces RawFileView with a real editor.
//
// We deliberately do *not* recurse upfront — `node_modules`-heavy
// projects would spend tens of seconds enumerating tens of thousands
// of files. Step-by-step navigation keeps each refresh cheap.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RawFileView } from "./RawFileView.tsx";
import { Splitter } from "./Splitter.tsx";
import { usePersistedState } from "./prefs.ts";

interface DirEntryDto {
	name: string;
	kind: "file" | "dir" | "symlink";
	size: number;
	mtimeSecs: number | null;
}

interface TextDto {
	content: string;
	truncated: boolean;
}

interface FileTreeProps {
	cwd: string;
	/** False while this body is hidden (inactive harness tab / room).
	 *  Hidden trees run no watcher and fetch nothing — a user with N
	 *  files harnesses must not pay N recursive watchers at boot. */
	visible: boolean;
}

const DIR_GLYPH = "▸";
const FILE_GLYPH = "·";
const SYMLINK_GLYPH = "↗";

const formatRelativeTime = (mtimeSecs: number | null): string => {
	if (mtimeSecs === null) return "";
	const nowSecs = Date.now() / 1000;
	const delta = Math.max(0, nowSecs - mtimeSecs);
	if (delta < 60) return "just now";
	if (delta < 3600) return `${Math.floor(delta / 60)}m`;
	if (delta < 86400) return `${Math.floor(delta / 3600)}h`;
	if (delta < 86400 * 30) return `${Math.floor(delta / 86400)}d`;
	return new Date(mtimeSecs * 1000).toISOString().slice(0, 10);
};

const formatSize = (bytes: number): string => {
	if (bytes < 1024) return `${bytes}b`;
	if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)}k`;
	return `${(bytes / (1024 * 1024)).toFixed(1)}M`;
};

export const FileTree = ({ cwd, visible }: FileTreeProps) => {
	// `subPath` is the path *relative* to cwd that we're currently
	// listing. Starts at "" (the cwd itself).
	const [subPath, setSubPath] = useState<string>("");
	const [entries, setEntries] = useState<DirEntryDto[] | null>(null);
	const [error, setError] = useState<string | null>(null);
	// Selected file (relative to current subPath) for the raw view.
	const [selectedFile, setSelectedFile] = useState<string | null>(null);
	const [fileText, setFileText] = useState<TextDto | null>(null);
	const [fileNote, setFileNote] = useState<string | null>(null);

	const currentDir = useMemo(() => joinPath(cwd, subPath), [cwd, subPath]);

	// Guards against a slow list_dir for an abandoned directory
	// resolving after a fast one for the current directory and
	// clobbering it (breadcrumb back-out during a big listing).
	const refreshSeq = useRef(0);
	const visibleRef = useRef(visible);
	visibleRef.current = visible;

	const refresh = useCallback(async () => {
		const seq = ++refreshSeq.current;
		setError(null);
		try {
			const list = await invoke<DirEntryDto[]>("list_dir", { path: currentDir });
			if (seq !== refreshSeq.current) return;
			// Hide hidden entries. Sort: dirs first, then by mtime
			// descending. Within the same kind, recent first.
			const filtered = list.filter((e) => !e.name.startsWith("."));
			filtered.sort((a, b) => {
				const aIsDir = a.kind === "dir";
				const bIsDir = b.kind === "dir";
				if (aIsDir !== bIsDir) return aIsDir ? -1 : 1;
				const am = a.mtimeSecs ?? 0;
				const bm = b.mtimeSecs ?? 0;
				return bm - am;
			});
			setEntries(filtered);
		} catch (err: unknown) {
			if (seq !== refreshSeq.current) return;
			const msg = err instanceof Error ? err.message : String(err);
			setError(msg);
			setEntries([]);
		}
	}, [currentDir]);

	// Reset selection when the directory changes — a file selected in
	// `src/` doesn't carry meaning when we navigate to `tests/`.
	// Fetching is skipped while hidden (navigation only happens while
	// visible anyway; the boot-time mount of a hidden files harness
	// must not fan out list_dir calls).
	useEffect(() => {
		setSelectedFile(null);
		setFileText(null);
		setFileNote(null);
		if (visibleRef.current) void refresh();
	}, [refresh]);

	// The watcher below is off while hidden, so refresh on every
	// show — the listing may be stale from agent activity meanwhile.
	useEffect(() => {
		if (visible) void refresh();
	}, [visible, refresh]);

	// Watch the *current* subdirectory so the listing reflects what
	// the agent (or anyone else) is doing in real time. Re-runs on
	// every navigation since the watched path changes — and only runs
	// while this body is visible.
	useEffect(() => {
		if (!visible) return;
		const channel = new Channel<null>();
		channel.onmessage = () => {
			void refresh();
		};

		let watchId: string | null = null;
		let cancelled = false;

		invoke<string>("git_watch_start", { path: currentDir, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] FileTree watch failed:", msg);
			});

		return () => {
			cancelled = true;
			if (watchId) {
				void invoke("git_watch_stop", { id: watchId });
			}
		};
	}, [currentDir, refresh, visible]);

	useEffect(() => {
		if (!selectedFile) {
			setFileText(null);
			setFileNote(null);
			return;
		}
		setFileText(null);
		setFileNote(null);
		const fullPath = joinPath(currentDir, selectedFile);
		let cancelled = false;
		invoke<TextDto>("read_file_text", { path: fullPath })
			.then((dto) => {
				if (!cancelled) setFileText(dto);
			})
			.catch((err: unknown) => {
				if (cancelled) return;
				const msg = err instanceof Error ? err.message : String(err);
				// `read_file_text` returns the conventional "binary"
				// error when the sniff window contains NULs.
				setFileNote(msg === "binary" ? "binary file — no text view" : `cannot open: ${msg}`);
			});
		return () => {
			cancelled = true;
		};
	}, [selectedFile, currentDir]);

	const breadcrumbSegments = useMemo(() => {
		// Build breadcrumb pieces from `subPath`. Each segment can be
		// clicked to jump back to that depth.
		const segments = subPath.split("/").filter(Boolean);
		return segments.map((name, i) => ({
			name,
			path: segments.slice(0, i + 1).join("/"),
		}));
	}, [subPath]);

	const onEntryClick = (entry: DirEntryDto) => {
		if (entry.kind === "dir") {
			setSubPath((prev) => (prev ? `${prev}/${entry.name}` : entry.name));
		} else if (entry.kind === "file") {
			setSelectedFile(entry.name === selectedFile ? null : entry.name);
		}
		// Symlinks are display-only for v1 — too easy to escape the cwd.
	};

	// Overlay layout is tree LEFT / raw view RIGHT (the old right-pane
	// incarnation stacked them vertically).
	const [treeWidth, setTreeWidth] = usePersistedState<number>("filesTreeWidth", 280);

	const listEl = (
		<div
			style={{
				flex: 1,
				overflowY: "auto",
				padding: "6px 0",
				fontFamily: "var(--sk-mono)",
				fontSize: 11,
				minHeight: 0,
			}}
		>
			{entries === null && !error && (
				<div style={{ padding: "10px 14px", color: "var(--fg-3)" }}>loading…</div>
			)}
			{entries !== null && entries.length === 0 && (
				<div
					style={{
						padding: "32px 14px",
						color: "var(--fg-3)",
						textAlign: "center",
					}}
				>
					empty folder
				</div>
			)}
			{entries?.map((e) => {
				const selected = e.kind === "file" && e.name === selectedFile;
				const glyph =
					e.kind === "dir" ? DIR_GLYPH : e.kind === "symlink" ? SYMLINK_GLYPH : FILE_GLYPH;
				const isDir = e.kind === "dir";
				return (
					<div
						key={e.name}
						onClick={() => onEntryClick(e)}
						style={{
							display: "flex",
							gap: 10,
							padding: "3px 14px",
							color: isDir ? "var(--fg-0)" : "var(--fg-1)",
							cursor: e.kind === "symlink" ? "default" : "pointer",
							background: selected ? "var(--bg-3)" : "transparent",
						}}
						title={e.kind === "symlink" ? "symlink (not navigable)" : undefined}
					>
						<span style={{ color: "var(--fg-3)", width: 14, textAlign: "center" }}>{glyph}</span>
						<span
							style={{
								flex: 1,
								minWidth: 0,
								overflow: "hidden",
								textOverflow: "ellipsis",
								fontWeight: isDir ? 500 : 400,
							}}
						>
							{e.name}
							{isDir ? "/" : ""}
						</span>
						<span style={{ color: "var(--fg-3)", fontSize: 10, minWidth: 36, textAlign: "right" }}>
							{e.kind === "file" ? formatSize(e.size) : ""}
						</span>
						<span style={{ color: "var(--fg-3)", fontSize: 10, minWidth: 56, textAlign: "right" }}>
							{formatRelativeTime(e.mtimeSecs)}
						</span>
					</div>
				);
			})}
			{error && <div style={{ padding: "10px 14px", color: "var(--err)" }}>error: {error}</div>}
		</div>
	);

	const viewerEl = (
		<div
			style={{
				flex: 1,
				overflowY: "auto",
				padding: selectedFile ? "0 14px 10px" : "10px 14px",
				fontFamily: "var(--sk-mono)",
				fontSize: 11,
				background: "var(--bg-0)",
				color: "var(--fg-1)",
				whiteSpace: "pre",
				minWidth: 0,
			}}
		>
			{!selectedFile && (
				<div style={{ color: "var(--fg-3)", padding: "24px 0", textAlign: "center" }}>
					select a file to view
				</div>
			)}
			{selectedFile && fileNote && (
				<div style={{ color: "var(--fg-3)", paddingTop: 10 }}>{fileNote}</div>
			)}
			{selectedFile && !fileNote && fileText === null && (
				<div style={{ color: "var(--fg-3)", paddingTop: 10 }}>loading…</div>
			)}
			{selectedFile && fileText !== null && (
				<RawFileView text={fileText.content} truncated={fileText.truncated} />
			)}
		</div>
	);

	return (
		<div
			style={{
				flex: 1,
				display: "flex",
				flexDirection: "column",
				minHeight: 0,
				minWidth: 0,
				background: "var(--bg-1)",
			}}
		>
			<div
				style={{
					padding: "10px 14px",
					borderBottom: "1px solid var(--line)",
					display: "flex",
					alignItems: "center",
					gap: 6,
					background: "var(--bg-0)",
					fontFamily: "var(--sk-mono)",
					fontSize: 11,
					color: "var(--fg-2)",
					flexWrap: "wrap",
				}}
			>
				<span style={{ color: "var(--fg-0)" }}>Files</span>
				<span style={{ color: "var(--fg-3)" }}>·</span>
				<span
					style={{
						cursor: "pointer",
						color: subPath === "" ? "var(--fg-1)" : "var(--accent)",
					}}
					onClick={() => setSubPath("")}
					title={cwd}
				>
					{cwdLabel(cwd)}
				</span>
				{breadcrumbSegments.map((seg) => (
					<span key={seg.path} style={{ display: "flex", alignItems: "center", gap: 6 }}>
						<span style={{ color: "var(--fg-3)" }}>/</span>
						<span
							style={{
								cursor: "pointer",
								color: seg.path === subPath ? "var(--fg-1)" : "var(--accent)",
							}}
							onClick={() => setSubPath(seg.path)}
						>
							{seg.name}
						</span>
					</span>
				))}
			</div>
			<Splitter
				direction="row"
				size={treeWidth}
				onResize={setTreeWidth}
				minFirst={180}
				minSecond={260}
				first={listEl}
				second={viewerEl}
			/>
		</div>
	);
};

const joinPath = (a: string, b: string): string => {
	if (!b) return a;
	const sep = a.includes("\\") && !a.includes("/") ? "\\" : "/";
	const trimmed = a.replace(/[\\/]+$/, "");
	return `${trimmed}${sep}${b.replace(/\//g, sep)}`;
};

const cwdLabel = (cwd: string): string => {
	const cleaned = cwd.replace(/[\\/]+$/, "");
	const parts = cleaned.split(/[\\/]/);
	return parts[parts.length - 1] || cleaned;
};
