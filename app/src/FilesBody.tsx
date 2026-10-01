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

import { invoke } from "@tauri-apps/api/core";
import { FileTree } from "./FileTree.tsx";
import { ImageView } from "./ImageView.tsx";
import { Splitter } from "./Splitter.tsx";
import { usePersistedState } from "./prefs.ts";
import { useFileBuffers } from "./useFileBuffers.ts";

interface FilesBodyProps {
	harnessId: string;
	cwd: string;
	visible: boolean;
}

export const FilesBody = ({ harnessId, cwd, visible }: FilesBodyProps) => {
	const [treeWidth, setTreeWidth] = usePersistedState<number>("filesTreeWidth", 280);
	const {
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
	} = useFileBuffers(harnessId, visible);

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
									onClick={() => selectTab(t.path)}
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
						{activeTab?.readOnly && !activeTab.isImage && (
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
						<div
							ref={hostRef}
							className="fp-code"
							style={{ display: active && !activeTab?.isImage ? "block" : "none" }}
						/>
						{active && activeTab?.isImage && (
							<div className="fp-code">
								<ImageView
									path={active}
									loadKey={`${active}#${imgRef.current.get(active) ?? 0}`}
									load={() => invoke<ArrayBuffer>("read_image_bytes", { path: active })}
								/>
							</div>
						)}
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
