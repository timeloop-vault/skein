// FilesOverlay — #49 stage 1: browse and view the room's files
// without leaving Skein. A large overlay hosting the resurrected
// FileTree (tree left, preview right). Read-only; the editor is
// stage 2. Opened via Mod+E or the command palette; Esc / backdrop
// click / Mod+E again close it.

import { useEffect, useRef } from "react";
import { FileTree } from "./FileTree.tsx";
import { useFocusRestore } from "./useFocusRestore.ts";

interface FilesOverlayProps {
	cwd: string;
	/** True while another overlay (palette, settings, …) is stacked on
	 *  top — Esc then belongs to that layer, not us. Window-listener
	 *  registration order makes `defaultPrevented` alone unreliable
	 *  here (ours registered first, so it fires first). */
	suspended: boolean;
	onClose: () => void;
}

export const FilesOverlay = ({ cwd, suspended, onClose }: FilesOverlayProps) => {
	useFocusRestore();
	const panelRef = useRef<HTMLDivElement | null>(null);

	// Take keyboard focus on open, like every other overlay. Without
	// this the xterm textarea keeps focus: typing leaks into the PTY
	// behind the backdrop, and xterm swallows Esc — worse, it FORWARDS
	// the ESC byte to the harness, which cancels a running agent turn.
	useEffect(() => {
		panelRef.current?.focus();
	}, []);

	useEffect(() => {
		if (suspended) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			// One Esc peels one layer: an overlay stacked above us that
			// already claimed this event marks it defaultPrevented (the
			// palette's input handler runs before window listeners).
			// The find-in-file input's query-clearing Esc uses
			// stopPropagation and never gets here at all.
			if (e.defaultPrevented) return;
			e.preventDefault();
			onClose();
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [onClose, suspended]);

	return (
		<div className="sk-files-backdrop" onClick={onClose}>
			<div
				ref={panelRef}
				tabIndex={-1}
				className="sk-files-panel"
				onClick={(e) => e.stopPropagation()}
			>
				<FileTree cwd={cwd} />
			</div>
		</div>
	);
};
