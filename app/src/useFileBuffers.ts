// Buffer/tab state for a `files` harness body (#185, #409): open, save,
// close, LRU eviction, and the dirty-registry wiring. Unsaved text lives
// only in memory, so every destroy path consults `filesRegistry`.

import type { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { createBufferState } from "./editor.ts";
import { filesRegistry } from "./filesRegistry.ts";
import { isImagePath } from "./imageFiles.ts";

export interface TextDto {
	content: string;
	truncated: boolean;
	/// Lossy UTF-8 decode — saving would destroy original bytes.
	lossy: boolean;
	/// Staleness token, round-tripped through write_file_text.
	mtime_ms: number;
}

export interface TabInfo {
	path: string; // absolute
	name: string; // leaf, for the tab + prompts
	dirty: boolean;
	readOnly: boolean;
	/** #409: an image preview tab — no CodeMirror buffer, never dirty,
	 *  rendered by ImageView instead of the fp-code host. */
	isImage: boolean;
}

/** Past this many open buffers, the least-recently-active CLEAN
 *  buffer is evicted on open. Dirty buffers are never evicted. */
const MAX_BUFFERS = 12;

export const useFileBuffers = (harnessId: string, visible: boolean) => {
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
	// path -> reload counter for open image tabs (#409). Mirrors bufRef's
	// role as the synchronous "is this already open" source of truth —
	// tabsRef lags a render behind setTabs, so a same-tick reopen (or a
	// double-click race, same hazard bufRef guards against below) can't
	// rely on it. Bumping the counter on a reopen is also ImageView's
	// reload trigger via loadKey.
	const imgRef = useRef(new Map<string, number>());
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
		if (activeRef.current && !imgRef.current.has(activeRef.current)) viewRef.current?.focus();
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

	/** Activate an image tab: no CodeMirror buffer to swap in, and the
	 *  CM view must give up focus so hidden keystrokes don't land in it
	 *  (#409). */
	const activateImage = (path: string) => {
		if (activeRef.current === path) return;
		stashActive();
		viewRef.current?.contentDOM.blur();
		activeRef.current = path;
		setActive(path);
		lruRef.current.set(path, ++lruSeq.current);
	};

	/** Dispatch to the right activation path by tab kind. `imgRef` is
	 *  consulted rather than `tabsRef` because it is updated
	 *  synchronously at tab creation, the same reason `bufRef` is used
	 *  for text tabs. */
	const selectTab = (path: string) => {
		if (imgRef.current.has(path)) activateImage(path);
		else activate(path);
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

	/** Evict the least-recently-active clean tab (any kind) past
	 *  MAX_BUFFERS, then append `tab`. Shared by the text and image open
	 *  paths so the LRU policy stays one piece of logic. */
	const pushTab = (tab: TabInfo) => {
		setTabs((prev) => {
			let next = prev;
			if (prev.length >= MAX_BUFFERS) {
				const clean = prev.filter((t) => !t.dirty && t.path !== activeRef.current);
				if (clean.length > 0) {
					const evict = clean.reduce((a, b) =>
						(lruRef.current.get(a.path) ?? 0) <= (lruRef.current.get(b.path) ?? 0) ? a : b,
					);
					bufRef.current.delete(evict.path);
					imgRef.current.delete(evict.path);
					lruRef.current.delete(evict.path);
					next = prev.filter((t) => t.path !== evict.path);
				}
			}
			return [...next, tab];
		});
	};

	/** Open (or reopen) an image tab (#409): no read_file_text, no
	 *  CodeMirror buffer — ImageView loads bytes itself, lazily, via
	 *  `read_image_bytes`. Reopening an already-open image bumps its
	 *  reload counter so ImageView's loadKey changes and it re-fetches. */
	const openImage = (absPath: string, name: string) => {
		if (imgRef.current.has(absPath)) {
			imgRef.current.set(absPath, (imgRef.current.get(absPath) ?? 0) + 1);
			activateImage(absPath);
			return;
		}
		imgRef.current.set(absPath, 0);
		pushTab({ path: absPath, name, dirty: false, readOnly: true, isImage: true });
		activateImage(absPath);
	};

	const openFile = async (absPath: string, name: string) => {
		if (isImagePath(absPath)) {
			openImage(absPath, name);
			return;
		}
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
			pushTab({ path: absPath, name, dirty: false, readOnly: viewOnly, isImage: false });
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
		imgRef.current.delete(path);
		lruRef.current.delete(path);
		setTabs(next);
		if (activeRef.current === path) {
			activeRef.current = null;
			const neighbor = next[Math.min(idx, next.length - 1)];
			if (neighbor) selectTab(neighbor.path);
			else setActive(null);
		}
	};

	return {
		tabs,
		active,
		note,
		setNote,
		pendingClose,
		setPendingClose,
		hostRef,
		saveRef,
		imgRef,
		selectTab,
		openFile,
		closeBuffer,
	};
};
