// FileTree — non-git room companion for `LiveStatus` (issue #7).
//
// One-level directory listing rooted at the room's cwd, navigable
// in-place: click a dir to descend, click a breadcrumb segment to go
// back up, click a file to see a text preview. Sorted by mtime
// descending so the things the agent just wrote bubble to the top.
//
// We deliberately do *not* recurse upfront — `node_modules`-heavy
// projects would spend tens of seconds enumerating tens of thousands
// of files. Step-by-step navigation keeps each refresh cheap.

import { Channel, invoke } from "@tauri-apps/api/core";
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { Splitter } from "./Splitter.tsx";
import { usePersistedState } from "./prefs.ts";
import { findPreviewProviders } from "./preview/index.ts";

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

interface BytesDto {
	base64: string;
	truncated: boolean;
}

interface FileTreeProps {
	cwd: string;
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

export const FileTree = ({ cwd }: FileTreeProps) => {
	// `subPath` is the path *relative* to cwd that we're currently
	// listing. Starts at "" (the cwd itself).
	const [subPath, setSubPath] = useState<string>("");
	const [entries, setEntries] = useState<DirEntryDto[] | null>(null);
	const [error, setError] = useState<string | null>(null);
	// Selected file (relative to current subPath) for preview.
	const [selectedFile, setSelectedFile] = useState<string | null>(null);
	const [previewBody, setPreviewBody] = useState<ReactNode | null>(null);
	const [previewError, setPreviewError] = useState<string | null>(null);

	const currentDir = useMemo(() => joinPath(cwd, subPath), [cwd, subPath]);

	const refresh = useCallback(async () => {
		setError(null);
		try {
			const list = await invoke<DirEntryDto[]>("list_dir", { path: currentDir });
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
			const msg = err instanceof Error ? err.message : String(err);
			setError(msg);
			setEntries([]);
		}
	}, [currentDir]);

	// Reset selection when the directory changes — a file selected in
	// `src/` doesn't carry meaning when we navigate to `tests/`.
	useEffect(() => {
		setSelectedFile(null);
		setPreviewBody(null);
		setPreviewError(null);
		void refresh();
	}, [refresh]);

	// Watch the *current* subdirectory so the listing reflects what
	// the agent (or anyone else) is doing in real time. Re-runs on
	// every navigation since the watched path changes.
	useEffect(() => {
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
	}, [currentDir, refresh]);

	useEffect(() => {
		if (!selectedFile) {
			setPreviewBody(null);
			setPreviewError(null);
			return;
		}
		setPreviewBody(null);
		setPreviewError(null);
		const fullPath = joinPath(currentDir, selectedFile);
		const providers = findPreviewProviders(fullPath);
		if (providers.length === 0) {
			setPreviewError("no preview provider matched");
			return;
		}
		let cancelled = false;
		const load = async () => {
			// Walk providers in priority order. A provider opts out by
			// returning `null` from render, or by having its underlying
			// fetch fail with the conventional `"binary"` error
			// (`read_file_text` does this for files with NULs in the
			// sniff window — the signal hex provider is waiting for).
			let lastError: string | null = null;
			for (const provider of providers) {
				if (cancelled) return;
				try {
					let body: ReactNode | null = null;
					if (provider.needs === "text") {
						const dto = await invoke<TextDto>("read_file_text", { path: fullPath });
						if (cancelled) return;
						body = provider.render({
							path: fullPath,
							text: dto.content,
							truncated: dto.truncated,
						});
					} else if (provider.needs === "bytes") {
						const dto = await invoke<BytesDto>("read_file_bytes", { path: fullPath });
						if (cancelled) return;
						body = provider.render({
							path: fullPath,
							bytesBase64: dto.base64,
							truncated: dto.truncated,
						});
					} else {
						// "path" providers do their own io.
						body = provider.render({ path: fullPath, truncated: false });
					}
					if (body !== null) {
						setPreviewBody(body);
						return;
					}
				} catch (err: unknown) {
					const msg = err instanceof Error ? err.message : String(err);
					// "binary" is the expected fall-through signal —
					// don't surface it unless every provider failed.
					lastError = msg;
				}
			}
			if (cancelled) return;
			setPreviewError(lastError ?? "no preview provider could render this file");
		};
		void load();
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

	const [previewHeight, setPreviewHeight] = usePersistedState<number>("fileTreePreviewHeight", 220);

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

	const previewEl = selectedFile ? (
		<div
			style={{
				flex: 1,
				overflowY: "auto",
				padding: "10px 14px",
				fontFamily: "var(--sk-mono)",
				fontSize: 11,
				background: "var(--bg-0)",
				color: "var(--fg-1)",
				whiteSpace: "pre",
			}}
		>
			{previewError && <div style={{ color: "var(--fg-3)" }}>cannot preview: {previewError}</div>}
			{!previewError && previewBody === null && (
				<div style={{ color: "var(--fg-3)" }}>loading…</div>
			)}
			{previewBody}
		</div>
	) : null;

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
			{selectedFile ? (
				<Splitter
					direction="column"
					size={previewHeight}
					onResize={setPreviewHeight}
					minFirst={120}
					minSecond={120}
					first={listEl}
					second={previewEl}
				/>
			) : (
				listEl
			)}
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
