// FilesBody — the body of a `files` harness (#49 phase A): the
// browse + raw-view surface occupying the slot a terminal would.
// Because harness bodies are mutually exclusive tabs, a visible
// Files body means no visible terminal — the overlay era's
// keystroke-leak class is impossible by construction.
//
// Phase B replaces the raw view with the CodeMirror editor and
// hangs per-harness buffer state here.

import { FileTree } from "./FileTree.tsx";

export const FilesBody = ({ cwd, visible }: { cwd: string; visible: boolean }) => {
	if (!cwd) {
		return <div className="sk-files-nocwd">this room has no folder to browse</div>;
	}
	return <FileTree cwd={cwd} visible={visible} />;
};
