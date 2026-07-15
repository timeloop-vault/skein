// FilesBody — the body of a `files` harness (#49 phase B): tree on
// the left, buffer tabs + CodeMirror editor on the right, in the
// slot a terminal would occupy. Because harness bodies are mutually
// exclusive tabs, a visible Files body means no visible terminal —
// keystrokes structurally can't leak into a PTY.
//
// Buffer model (design handover §7/§8): multiple open files per
// Files harness; one EditorView per body with one EditorState per
// buffer (undo history and cursor survive tab switches); dirty ●
// replaces the close ×; reopening a file focuses its buffer; LRU
// eviction of clean buffers past MAX_BUFFERS. Mod+S saves via the
// room-scoped `write_file_text`; truncated large files open
// view-only (saving a truncated read would destroy the file's
// tail). Unsaved text exists only in memory — the dirty registry
// lets App prompt before any destroy path (harness/room close,
// quit).

import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { FileTree } from "./FileTree.tsx";
import { Splitter } from "./Splitter.tsx";
import { createBufferState } from "./editor.ts";
import { filesRegistry } from "./filesRegistry.ts";
import { usePersistedState } from "./prefs.ts";

interface TextDto {
	content: string;
	truncated: boolean;
	/// Lossy UTF-8 decode — saving would destroy original bytes.
	lossy: boolean;
	/// Staleness token, round-tripped through write_file_text.
	mtime_ms: number;
}

interface TabInfo {
	path: string; // absolute
	name: string; // leaf, for the tab + prompts
	dirty: boolean;
	readOnly: boolean;
}

/** Past this many open buffers, the least-recently-active CLEAN
 *  buffer is evicted on open. Dirty buffers are never evicted. */
const MAX_BUFFERS = 12;

interface FilesBodyProps {
	harnessId: string;
	cwd: string;
	visible: boolean;
}

export const FilesBody = ({ harnessId, cwd, visible }: FilesBodyProps) => {
	const [treeWidth, setTreeWidth] = usePersistedState<number>("filesTreeWidth", 280);
	const [tabs, setTabs] = useState<TabInfo[]>([]);
	const [active, setActive] = useState<string | null>(null);
	// Transient notice (open/save failures, binary files). Click to
	// dismiss; replaced by the next event.
	const [note, setNote] = useState<string | null>(null);
	// Path awaiting the Save / Discard / Cancel decision.
	const [pendingClose, setPendingClose] = useState<string | null>(null);

	const bufRef = useRef(
		new Map<string, { state: EditorState; readOnly: boolean; mtimeMs: number }>(),
	);
	// In-flight guards: double-click opens and Mod+S mashes race the
	// awaited IPC round-trips (#185 review).
	const openingRef = useRef(new Set<string>());
	const savingRef = useRef(new Set<string>());
	const lruRef = useRef(new Map<string, number>());
	const lruSeq = useRef(0);
	const viewRef = useRef<EditorView | null>(null);
	const hostRef = useRef<HTMLDivElement | null>(null);
	const activeRef = useRef<string | null>(null);
	const tabsRef = useRef(tabs);
	tabsRef.current = tabs;

	// Dirty registry — App consults this before destroy paths.
	useEffect(
		() =>
			filesRegistry.register(harnessId, {
				dirtyNames: () => tabsRef.current.filter((t) => t.dirty).map((t) => t.name),
			}),
		[harnessId],
	);

	// One EditorView for the body's lifetime; buffers swap via setState.
	useEffect(() => {
		const host = hostRef.current;
		if (!host) return;
		const view = new EditorView({ parent: host });
		viewRef.current = view;
		return () => {
			view.destroy();
			viewRef.current = null;
		};
	}, []);

	// Hidden bodies keep their DOM; re-measure + refocus on show so
	// CM's geometry is right after display:none.
	useEffect(() => {
		if (!visible) return;
		viewRef.current?.requestMeasure();
		if (activeRef.current) viewRef.current?.focus();
	}, [visible]);

	/** Persist the live view state back into the buffer map. */
	const stashActive = () => {
		const v = viewRef.current;
		const a = activeRef.current;
		if (!v || !a) return;
		const b = bufRef.current.get(a);
		if (b) b.state = v.state;
	};

	const activate = (path: string) => {
		const v = viewRef.current;
		const b = bufRef.current.get(path);
		if (!v || !b || activeRef.current === path) {
			v?.focus();
			return;
		}
		stashActive();
		v.setState(b.state);
		activeRef.current = path;
		setActive(path);
		lruRef.current.set(path, ++lruSeq.current);
		v.focus();
	};

	const markDirty = (path: string) => {
		// Only flip false→true — a per-keystroke setState would re-render
		// the whole body for nothing.
		setTabs((prev) =>
			prev.some((t) => t.path === path && !t.dirty)
				? prev.map((t) => (t.path === path ? { ...t, dirty: true } : t))
				: prev,
		);
	};

	const save = async (path: string): Promise<boolean> => {
		stashActive();
		const buf = bufRef.current.get(path);
		if (!buf || buf.readOnly) return false;
		// A clean buffer must never be written back: an unedited save
		// still rewrites bytes (and a lossy/normalized read would
		// destroy the original file). No edit → nothing to do.
		const tab = tabsRef.current.find((t) => t.path === path);
		if (!tab?.dirty) return true;
		if (savingRef.current.has(path)) return false;
		savingRef.current.add(path);
		// Only keystrokes up to THIS doc are being persisted — clearing
		// dirty after the await must not swallow typing that happened
		// during the write (Text is immutable; identity compare works).
		const savedDoc = buf.state.doc;
		try {
			const newMtime = await invoke<number>("write_file_text", {
				path,
				content: buf.state.sliceDoc(0),
				expectedMtimeMs: buf.mtimeMs,
			});
			buf.mtimeMs = newMtime;
			stashActive();
			const current = bufRef.current.get(path)?.state.doc;
			if (current === savedDoc) {
				setTabs((prev) => prev.map((t) => (t.path === path ? { ...t, dirty: false } : t)));
			}
			setNote(null);
			return true;
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			setNote(
				msg.includes("changed on disk")
					? `${tab.name}: changed on disk since you opened it — close the tab and reopen to pick up the new version (live reload is phase D)`
					: `save failed: ${msg}`,
			);
			return false;
		} finally {
			savingRef.current.delete(path);
		}
	};
	const saveRef = useRef(save);
	saveRef.current = save;

	const openFile = async (absPath: string, name: string) => {
		if (bufRef.current.has(absPath)) {
			activate(absPath);
			return;
		}
		if (openingRef.current.has(absPath)) return;
		openingRef.current.add(absPath);
		try {
			const dto = await invoke<TextDto>("read_file_text", { path: absPath });
			// Re-check after the await — a double-click races two opens.
			if (bufRef.current.has(absPath)) {
				activate(absPath);
				return;
			}
			const viewOnly = dto.truncated || dto.lossy;
			const state = createBufferState(dto.content, absPath, viewOnly, {
				onDocChanged: () => markDirty(absPath),
				onSave: () => void saveRef.current(absPath),
			});
			bufRef.current.set(absPath, { state, readOnly: viewOnly, mtimeMs: dto.mtime_ms });
			setTabs((prev) => {
				let next = prev;
				if (prev.length >= MAX_BUFFERS) {
					const clean = prev.filter((t) => !t.dirty && t.path !== activeRef.current);
					if (clean.length > 0) {
						const evict = clean.reduce((a, b) =>
							(lruRef.current.get(a.path) ?? 0) <= (lruRef.current.get(b.path) ?? 0) ? a : b,
						);
						bufRef.current.delete(evict.path);
						lruRef.current.delete(evict.path);
						next = prev.filter((t) => t.path !== evict.path);
					}
				}
				return [...next, { path: absPath, name, dirty: false, readOnly: viewOnly }];
			});
			activate(absPath);
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			setNote(
				msg === "binary" ? `${name}: binary file — no editor view` : `cannot open ${name}: ${msg}`,
			);
		} finally {
			openingRef.current.delete(absPath);
		}
	};

	const closeBuffer = (path: string, force: boolean) => {
		const cur = tabsRef.current;
		const tab = cur.find((t) => t.path === path);
		if (!tab) return;
		if (tab.dirty && !force) {
			setPendingClose(path);
			return;
		}
		const idx = cur.findIndex((t) => t.path === path);
		const next = cur.filter((t) => t.path !== path);
		bufRef.current.delete(path);
		lruRef.current.delete(path);
		setTabs(next);
		if (activeRef.current === path) {
			activeRef.current = null;
			const neighbor = next[Math.min(idx, next.length - 1)];
			if (neighbor) activate(neighbor.path);
			else setActive(null);
		}
	};

	const activeTab = tabs.find((t) => t.path === active) ?? null;
	const pendingTab = pendingClose ? (tabs.find((t) => t.path === pendingClose) ?? null) : null;

	if (!cwd) {
		return <div className="sk-files-nocwd">this room has no folder to browse</div>;
	}

	return (
		<div className="sk-files-body">
			<Splitter
				direction="row"
				size={treeWidth}
				onResize={setTreeWidth}
				minFirst={180}
				minSecond={320}
				first={
					<FileTree
						cwd={cwd}
						visible={visible}
						activePath={active}
						onOpenFile={(p, n) => void openFile(p, n)}
					/>
				}
				second={
					<div className="fp-editor-col">
						<div className="fp-buftabs">
							{tabs.map((t) => (
								<div
									key={t.path}
									className={`fp-buftab ${t.path === active ? "active" : ""}`}
									title={t.path}
									onClick={() => activate(t.path)}
								>
									<span className="name">{t.name}</span>
									{t.dirty ? (
										<span
											className="dot"
											title="unsaved changes — click to close"
											onClick={(e) => {
												e.stopPropagation();
												closeBuffer(t.path, false);
											}}
										>
											●
										</span>
									) : (
										<span
											className="x"
											onClick={(e) => {
												e.stopPropagation();
												closeBuffer(t.path, false);
											}}
										>
											×
										</span>
									)}
								</div>
							))}
						</div>
						{activeTab?.readOnly && (
							<div className="fp-notice">
								view only — file is over 256 KB or not valid UTF-8 (saving would mangle it)
							</div>
						)}
						{note && (
							<div
								className="fp-notice warn"
								title="click to dismiss"
								onClick={() => setNote(null)}
							>
								{note}
							</div>
						)}
						<div ref={hostRef} className="fp-code" style={{ display: active ? "block" : "none" }} />
						{!active && (
							<div className="fp-empty">
								<div className="glyph">◇</div>
								<div className="title">No file open</div>
								<div className="hint">pick a file in the tree</div>
							</div>
						)}
						{pendingTab && (
							<div className="fp-confirm">
								<span>
									Save changes to <strong>{pendingTab.name}</strong>?
								</span>
								<button
									type="button"
									className="sk-btn primary"
									onClick={() => {
										void saveRef.current(pendingTab.path).then((ok) => {
											if (ok) closeBuffer(pendingTab.path, true);
											setPendingClose(null);
										});
									}}
								>
									Save
								</button>
								<button
									type="button"
									className="sk-btn"
									onClick={() => {
										closeBuffer(pendingTab.path, true);
										setPendingClose(null);
									}}
								>
									Discard
								</button>
								<button
									type="button"
									className="sk-btn ghost"
									onClick={() => setPendingClose(null)}
								>
									Cancel
								</button>
							</div>
						)}
					</div>
				}
			/>
		</div>
	);
};
