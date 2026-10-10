// designCapture — the two requests a screenshot needs from the design pane
// (#552): `design_capture_target` (where the preview is painted, in the main
// webview's CSS px; never changes any UI) and `show_design_pane` (make it
// painted, switching room if the agent's permission allows). The decisions
// are pure; the DOM reads live in useDesignControl.

import { asRecord, type RequestResult } from "./agentRequestsShared.ts";
import type { DesignPaneApi } from "./designControl.ts";
import { REVEAL_TIMEOUT_MS } from "./designReveal.ts";

export type CaptureRect = { x: number; y: number; width: number; height: number };

export type NotVisibleReason = "other_room" | "hidden_tab" | "not_laid_out" | "no_pane";

/** The part of `rect` inside every clip rect (clipping ancestors, viewport),
 *  or null when under 1px is left in either dimension. */
export const intersectClips = (
	rect: CaptureRect,
	clips: readonly CaptureRect[],
): CaptureRect | null => {
	let left = rect.x;
	let top = rect.y;
	let right = rect.x + rect.width;
	let bottom = rect.y + rect.height;
	for (const c of clips) {
		left = Math.max(left, c.x);
		top = Math.max(top, c.y);
		right = Math.min(right, c.x + c.width);
		bottom = Math.min(bottom, c.y + c.height);
	}
	if (right - left < 1 || bottom - top < 1) return null;
	return { x: left, y: top, width: right - left, height: bottom - top };
};

/** What the mounted pane reports about its own frame, raw. `rect` is already
 *  clipped to what is on screen; `cssHidden` covers the frame and ancestors
 *  (`visibility: hidden|collapse`, `opacity: 0`). */
export interface PaneCaptureRaw {
	/** The pane's `visible` prop: its tab / harness is the one shown. */
	visible: boolean;
	/** The frame has a non-zero layout box. */
	laidOut: boolean;
	rect: CaptureRect | null;
	/** `visibility: hidden|collapse` on the frame (display:none shows as 0x0). */
	cssHidden: boolean;
	devicePixelRatio: number;
}

export type CaptureTargetResult = {
	visible: boolean;
	rect?: CaptureRect;
	devicePixelRatio: number;
	entry?: string | null;
	reason?: NotVisibleReason;
};

/** Why the pane is not painted, or null when it is. Order: the room first
 *  (nothing of an inactive room is on screen), then the tab, then layout. */
export const notVisibleReason = (
	activeRoomId: string | null,
	roomId: string,
	raw: PaneCaptureRaw,
): NotVisibleReason | null => {
	if (activeRoomId !== roomId) return "other_room";
	if (!raw.visible || raw.cssHidden) return "hidden_tab";
	const r = raw.rect;
	if (!raw.laidOut || !r || r.width <= 0 || r.height <= 0) return "not_laid_out";
	return null;
};

export function captureTargetResult(
	activeRoomId: string | null,
	roomId: string,
	raw: PaneCaptureRaw,
	entry: string | null,
): CaptureTargetResult {
	const reason = notVisibleReason(activeRoomId, roomId, raw);
	if (reason !== null || !raw.rect) {
		return {
			visible: false,
			devicePixelRatio: raw.devicePixelRatio,
			reason: reason ?? "not_laid_out",
		};
	}
	return { visible: true, rect: raw.rect, devicePixelRatio: raw.devicePixelRatio, entry };
}

export const NO_PANE_RESULT = (devicePixelRatio: number): CaptureTargetResult => ({
	visible: false,
	devicePixelRatio,
	reason: "no_pane",
});

export function parseHarnessIdArgs(raw: unknown): RequestResult<{ harnessId: string }> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "bad_arguments: args must be an object" };
	const harnessId = r.harnessId;
	if (typeof harnessId !== "string" || !harnessId) {
		return { ok: false, error: "bad_arguments: harnessId is required" };
	}
	return { ok: true, value: { harnessId } };
}

// ---- show_design_pane -----------------------------------------------------

export type ShownHow = "already" | "show_docked" | "switch" | "room";

export type ShowPaneResult = {
	shown: boolean;
	switchedRoom: boolean;
	revealed: ShownHow;
	reason?: string;
};

/** How to make a pane visible. Unlike #549's `revealDecision`, another room
 *  is switched to (the agent's MCP permission is the gate); `room` reports
 *  that the room changed, and the harness is still brought forward after. */
export const showPaneDecision = (
	activeRoomId: string | null,
	roomId: string,
	docked: boolean,
	alreadyVisible: boolean,
): { how: ShownHow; switchRoom: boolean; harness: "switch" | "show_docked" | null } => {
	const harness = docked ? "show_docked" : "switch";
	if (activeRoomId !== roomId) return { how: "room", switchRoom: true, harness };
	if (alreadyVisible) return { how: "already", switchRoom: false, harness: null };
	return { how: harness, switchRoom: false, harness };
};

/** What the handler needs from the app to show a pane. */
export interface ShowPaneActions {
	activeRoomId(): string | null;
	switchRoom(roomId: string): void;
	switchHarness(roomId: string, harnessId: string): void;
	isDocked(harnessId: string): boolean;
	showDocked(roomId: string, harnessId: string): void;
}

export async function showDesignPane(
	pane: Pick<DesignPaneApi, "roomId" | "whenVisible">,
	actions: ShowPaneActions,
	harnessId: string,
): Promise<ShowPaneResult> {
	const roomId = pane.roomId;
	const active = actions.activeRoomId();
	// A zero timeout is a single synchronous check of the predicate.
	const already = active === roomId && (await pane.whenVisible(0));
	const d = showPaneDecision(active, roomId, actions.isDocked(harnessId), already);
	if (d.switchRoom) actions.switchRoom(roomId);
	if (d.harness === "show_docked") actions.showDocked(roomId, harnessId);
	else if (d.harness === "switch") actions.switchHarness(roomId, harnessId);
	const shown = d.how === "already" ? true : await pane.whenVisible(REVEAL_TIMEOUT_MS);
	return {
		shown,
		switchedRoom: d.switchRoom,
		revealed: d.how,
		...(shown ? {} : { reason: "timeout: the pane did not become visible in time" }),
	};
}
