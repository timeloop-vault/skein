// The in-app confirm dialog (#242), replacing the three native
// `confirm()` calls from `@tauri-apps/plugin-dialog` (close room,
// close a Files harness with unsaved buffers, quit with unsaved
// buffers). `ConfirmDialogHost` is mounted once, at the end of
// AppOverlays.tsx's fragment so it paints above every other overlay
// sharing the same `.sk-modal-bg` z-index (later DOM siblings win the
// paint order at equal z-index).
//
// It renders the head of confirmDialog.ts's FIFO queue. Each request
// gets its own `ConfirmDialogBox`, keyed by `request.id`, so a fresh
// mount owns its own focus-management state rather than reusing the
// previous question's.

import { useEffect, useRef, useSyncExternalStore } from "react";
import type { ConfirmDialogRequest } from "./confirmDialog.ts";
import { getSnapshot, resolveConfirmDialog, subscribe } from "./confirmDialog.ts";
import { useFocusRestore } from "./useFocusRestore.ts";

export const ConfirmDialogHost = () => {
	const request = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
	if (!request) return null;
	return <ConfirmDialogBox key={request.id} request={request} />;
};

const ConfirmDialogBox = ({ request }: { request: ConfirmDialogRequest }) => {
	// Returns focus to the terminal once this box unmounts — whether
	// that's this request being answered, or the queue advancing past
	// it to the next one.
	useFocusRestore();

	const cancelRef = useRef<HTMLButtonElement>(null);
	const confirmRef = useRef<HTMLButtonElement>(null);
	// Focus the CANCEL button — a stray Enter should cancel, not
	// confirm a destructive action; Tab then Enter confirms. A plain
	// `autoFocus` prop only fires once and doesn't survive dev
	// StrictMode's synthetic mount→cleanup→mount: that cleanup runs
	// `useFocusRestore`'s dispatch immediately, which hands focus to
	// the terminal (see components.tsx's RoomNameInput comment for the
	// general shape of this trap) — but because this is a *second*,
	// independent effect, StrictMode's second pass re-runs it too and
	// takes focus right back, so the button ends up focused either way.
	useEffect(() => {
		cancelRef.current?.focus();
	}, []);

	const settle = (ok: boolean) => resolveConfirmDialog(request.id, ok);

	return (
		<div
			className="sk-modal-bg"
			onClick={() => settle(false)}
			onKeyDown={(e) => {
				// Never let a keystroke meant for this dialog reach the
				// window-level shortcut dispatch (useKeyboardShortcuts.ts) —
				// same reasoning as RoomNameInput in components.tsx.
				e.stopPropagation();
				if (e.key === "Escape") {
					e.preventDefault();
					settle(false);
					return;
				}
				// Trap Tab between the two buttons — only two focusable
				// elements exist in here, so wrapping Tab/Shift+Tab between
				// them is the whole story. Without this, Tab off the last
				// button escapes the dialog into the terminal underneath,
				// where a keystroke (e.g. Mod+W) bypasses the
				// stopPropagation above and can queue a second confirm
				// behind this one.
				if (e.key === "Tab") {
					if (e.shiftKey && document.activeElement === cancelRef.current) {
						e.preventDefault();
						confirmRef.current?.focus();
					} else if (!e.shiftKey && document.activeElement === confirmRef.current) {
						e.preventDefault();
						cancelRef.current?.focus();
					}
				}
			}}
		>
			<div
				className={`sk-modal sk-confirm-modal${request.kind === "warning" ? " warning" : ""}`}
				onClick={(e) => e.stopPropagation()}
			>
				<div className="sk-modal-head">
					<h2>{request.title}</h2>
				</div>
				<div className="sk-modal-body">
					<div className="sk-help">{request.message}</div>
				</div>
				<div className="sk-modal-foot">
					<button ref={cancelRef} type="button" className="sk-btn" onClick={() => settle(false)}>
						{request.cancelLabel ?? "Cancel"}
					</button>
					<button
						ref={confirmRef}
						type="button"
						className="sk-btn primary"
						onClick={() => settle(true)}
					>
						{request.confirmLabel ?? "OK"}
					</button>
				</div>
			</div>
		</div>
	);
};
