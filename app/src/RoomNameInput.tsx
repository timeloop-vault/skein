import { type KeyboardEvent as ReactKeyboardEvent, useEffect, useRef, useState } from "react";
import { commitRoomName } from "./roomName.ts";
import { OVERLAY_CLOSED_EVENT } from "./useFocusRestore.ts";

// #241: the room tab's name span turns into this on a double-click.
// No keystroke/pointer event may reach the tab underneath — a bare
// keydown would otherwise re-enter the window-level shortcut dispatch
// (App.tsx's `onKey`), and pointerdown/click/dblclick would start a tab
// drag (#271) or select/close the tab.
//
// Focus goes back to the terminal on Enter/Escape only, by dispatching
// `skein:overlay-closed` from those handlers — deliberately NOT via
// `useFocusRestore`, whose unmount-cleanup dispatch is wrong here twice
// over: (1) dev StrictMode runs every effect cleanup once right after
// mount, so the terminal grabbed focus, the input blurred, and blur's
// commit closed the editor the instant it opened; (2) a blur commit
// means the user clicked somewhere else on purpose, and yanking focus
// to the terminal would fight that click.
// Exported so `RoomStrip.tsx`'s `GroupTab` can reuse it directly — the
// top-row group tab renames the SAME `Room.name` field as the lead's
// own second-row tab (#241), just via a different host component.
export const RoomNameInput = ({
	initial,
	onCommit,
	onCancel,
}: {
	initial: string;
	onCommit: (name: string) => void;
	onCancel: () => void;
}) => {
	const [value, setValue] = useState(initial);
	const inputRef = useRef<HTMLInputElement>(null);
	// Enter/blur both commit; Escape cancels. Guards against firing
	// both (Enter's commit unmounts this input, which then blurs).
	const doneRef = useRef(false);

	// Synchronous focus/select on mount, deliberately not deferred to a
	// rAF/setTimeout: the command-palette-invoked path (App.tsx's
	// "Rename room") sets `renamingRoomId` and closes the palette in the
	// same event handler, so this component mounts in the same commit as
	// `CommandPalette` unmounts. React runs every passive-effect cleanup
	// in a commit (including `CommandPalette`'s `useFocusRestore`, which
	// focuses the terminal) before any passive-effect setup in that same
	// commit — so as long as this effect fires here and not later, it
	// runs after the terminal steals focus and wins the tug-of-war.
	useEffect(() => {
		const el = inputRef.current;
		if (!el) return;
		el.focus();
		el.select();
	}, []);

	const commit = () => {
		if (doneRef.current) return;
		doneRef.current = true;
		onCommit(commitRoomName(initial, value));
	};
	const cancel = () => {
		if (doneRef.current) return;
		doneRef.current = true;
		onCancel();
	};

	return (
		<input
			ref={inputRef}
			className="name name-input"
			value={value}
			onChange={(e) => setValue(e.target.value)}
			onBlur={commit}
			// #271: React dispatches bubbling synthetic events target-first,
			// so this fires before the tab root's own onPointerDown — the
			// stopPropagation here reaches (and short-circuits) `startDrag`
			// before it can call `setPointerCapture`, so clicking inside the
			// input to move the caret never lets the root capture the
			// pointer in the first place. click/dblclick are stopped for the
			// same reason: no accidental select/close/re-trigger-rename
			// while editing.
			onPointerDown={(e) => e.stopPropagation()}
			onClick={(e) => e.stopPropagation()}
			onDoubleClick={(e) => e.stopPropagation()}
			onKeyDown={(e: ReactKeyboardEvent<HTMLInputElement>) => {
				e.stopPropagation();
				if (e.key === "Enter") {
					e.preventDefault();
					commit();
					window.dispatchEvent(new Event(OVERLAY_CLOSED_EVENT));
				} else if (e.key === "Escape") {
					e.preventDefault();
					cancel();
					window.dispatchEvent(new Event(OVERLAY_CLOSED_EVENT));
				}
			}}
		/>
	);
};
