import { sendNotification } from "@choochmeque/tauri-plugin-notifications-api";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import { HChip } from "./components.tsx";
import { isWindows } from "./shortcuts.ts";
import type { ToastEntry } from "./toastStack.ts";
import { toastRemainingMs } from "./toastStack.ts";

// ── Toasts (in-app notifications, L5c) ─────────────────────────────
//
// A toast is the in-app complement to L5b's OS notification: it
// fires when Skein has focus but the user isn't looking at the
// source harness (e.g. they're in a different room when an agent
// finishes). Fixed to the bottom-right corner, click to jump,
// auto-dismiss after a few seconds.
//
// `ToastEntry`, `NewToast`, `TOAST_DISMISS_MS` and `TOAST_MAX_VISIBLE`
// live in `toastStack.ts` (#180) — a pure module so the stack's
// append/coalesce/expiry rules are testable without React. Re-exported
// here so existing importers keep working.

export type { NewToast, ToastEntry } from "./toastStack.ts";
export { TOAST_DISMISS_MS, TOAST_MAX_VISIBLE } from "./toastStack.ts";

/// api_error rows within this window count as one incident (a retry
/// burst lands as several rows seconds apart — badge once, not per row).
export const API_ERROR_INCIDENT_MS = 60_000;
/// #84: the notification plugin (Swift) crashed (use-after-free in
/// `saveNotification`) after long uptime — its `show` isn't safe to call
/// concurrently, and multiple harness transitions while the user is away
/// fire `sendNotification` from overlapping async tasks. Serialize every
/// OS notification through one promise chain so at most one is ever in
/// flight, removing the concurrency the race needs. Each link swallows
/// its own rejection so one failure (e.g. plugin absent in dev) doesn't
/// stall the chain.
let osNotifyChain: Promise<unknown> = Promise.resolve();
/// Monotonic 32-bit id per notification (the plugin requires a 32-bit
/// int). On macOS the native plugin drops the `extra` payload but DOES
/// round-trip this id to the click event (verified via skein.log, #118),
/// so we key the jump target off the id instead of `extra`.
let osNotifyId = 0;
/// id → where-to-jump, populated at send time and consumed on click.
/// Bounded so a long-running session can't grow it unboundedly; ids are
/// monotonic so the oldest insertion is the first key.
export const osNotifyTargets = new Map<number, { roomId: string; harnessId: string }>();
const OS_NOTIFY_TARGETS_MAX = 100;
/// `extra` rides along as the notification's payload and comes back via
/// `onNotificationClicked` so a click can jump to the harness that fired
/// it (#118). Values must be strings (the click data is Record<string,
/// string>).
export const enqueueOsNotification = (
	title: string,
	body: string,
	extra?: { roomId: string; harnessId: string },
): void => {
	osNotifyId = (osNotifyId + 1) % 0x7fff_ffff;
	const id = osNotifyId;
	// #294: Windows no longer round-trips this numeric id at all — the
	// click target is stored Rust-side, keyed off the toast itself, and
	// handed back via `os_notify_take_pending` (a store that survives
	// Skein being closed at click time). Only macOS's plugin still needs
	// the id-keyed map, since it drops `extra` but echoes the id.
	if (extra && !isWindows) {
		osNotifyTargets.set(id, extra);
		if (osNotifyTargets.size > OS_NOTIFY_TARGETS_MAX) {
			const oldest = osNotifyTargets.keys().next().value;
			if (oldest !== undefined) osNotifyTargets.delete(oldest);
		}
	}
	osNotifyChain = osNotifyChain
		.catch(() => {})
		.then(() => {
			// #155: the plugin's notify-rust backend never fires
			// `onNotificationClicked` on Windows (fire-and-forget —
			// see `os_notify.rs`), so Windows toasts go through our
			// own command instead, which wires up a real click.
			if (isWindows) {
				if (!extra) {
					console.warn("[skein] os_notify_show skipped: no roomId/harnessId (#294)");
					return undefined;
				}
				return invoke("os_notify_show", {
					roomId: extra.roomId,
					harnessId: extra.harnessId,
					title,
					body,
				});
			}
			return sendNotification(extra ? { id, title, body, extra } : { title, body });
		})
		.catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn("[skein] os notification failed:", msg);
		});
};

/// Per-harness badge coalesce window. A burst of badge-worthy
/// transitions inside this window (a Claude JSONL truncation-replay
/// re-emitting old end_turns — #62; shell prompt-redraw chatter
/// flipping running↔idle — #64) only bumps the count once. Genuine
/// activity spaced further apart than this still increments, and a
/// harness with no pending badge always shows the first one. The
/// underlying replay/dedup at the source is tracked in #93.
export const BADGE_COALESCE_MS = 10_000;

export const Toast = ({
	toast,
	onClick,
	onDismiss,
}: {
	toast: ToastEntry;
	onClick: () => void;
	onDismiss: () => void;
}) => {
	// #180: the timer runs from `toast.expiresAt` — fixed when the toast
	// entered the stack — not from whenever this effect happens to fire.
	// `onDismiss` is a fresh closure every App re-render (busy activity
	// under load re-renders constantly), so it's read through a ref
	// rather than a dep: previously it WAS the dep, which restarted the
	// timeout on every render and meant a toast never actually expired
	// under load.
	const onDismissRef = useRef(onDismiss);
	onDismissRef.current = onDismiss;
	useEffect(() => {
		const id = setTimeout(
			() => onDismissRef.current(),
			toastRemainingMs(toast.expiresAt, Date.now()),
		);
		return () => clearTimeout(id);
	}, [toast.expiresAt]);
	// #86: "permission" reads as "needs permission" (+ tool when known)
	// rather than the bare phase word — same reasoning as `statusLabel`,
	// just phrased for a subtitle instead of a status-bar segment.
	// #298: the subagent name (when known) joins the tool name.
	const permissionParts = [toast.agentType, toast.tool].filter((p): p is string => p !== undefined);
	const sub =
		toast.state === "permission"
			? `needs permission${permissionParts.length > 0 ? ` · ${permissionParts.join(" · ")}` : ""}`
			: // #277: "waiting · 3 delegated agents finished" when the
				// end-of-turn was withheld for delegated work.
				toast.state === "waiting" && toast.delegationNote
				? `${toast.state} · ${toast.delegationNote}`
				: toast.state === "created"
					? "opened"
					: toast.state;
	// #330: the "created" variant reads as a receipt for the room that
	// asked, not as an activity update on the harness itself — its title
	// names both rooms rather than just the target.
	const title =
		toast.state === "created" && toast.requesterRoomName
			? `${toast.requesterRoomName} opened ${toast.roomName}`
			: toast.roomName;
	return (
		<div
			className={`sk-toast${
				toast.state === "error"
					? " error"
					: toast.state === "permission"
						? " permission"
						: toast.state === "created"
							? " created"
							: ""
			}`}
			onClick={onClick}
			title="Go to this harness"
		>
			<HChip kind={toast.kind} />
			<div className="sk-toast-body">
				<div className="sk-toast-title">{title}</div>
				<div className="sk-toast-sub">
					{toast.harnessName} · {sub}
				</div>
				{toast.detail && <div className="sk-toast-detail">{toast.detail}</div>}
			</div>
			<span
				className="sk-toast-x"
				title="Dismiss"
				onClick={(e) => {
					e.stopPropagation();
					onDismiss();
				}}
			>
				×
			</span>
		</div>
	);
};
