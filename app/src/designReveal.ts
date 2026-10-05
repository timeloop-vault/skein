// designReveal — the opt-in `reveal` of show_element / invoke_element (#549).
// A room's agent harness and design harness share one column, so the driven
// pane is usually display:none (0x0 layout, throttled timers). Reveal
// switches the room's active harness to the design pane, but only when that
// room is already in front: it never switches rooms and never raises the
// window.

import type { DesignPaneApi } from "./designControl.ts";

export const REVEAL_TIMEOUT_MS = 1500;
const POLL_MS = 25;

/** What the request handler needs from the app to reveal a pane. */
export interface DesignReveal {
	activeRoomId(): string | null;
	switchHarness(roomId: string, harnessId: string): void;
	/** #551: is the design harness docked in its room's right pane? */
	isDocked(harnessId: string): boolean;
	/** #551: show a docked harness in the room's right-pane design tab. */
	showDocked(roomId: string, harnessId: string): void;
}

/** Only a room already in front may be revealed in. A docked harness is
 *  shown in the right pane (the active harness is left alone); otherwise
 *  the room's active harness is switched. Nothing is ever docked. */
export const revealDecision = (
	activeRoomId: string | null,
	roomId: string,
	docked = false,
): "switch" | "show_docked" | "other_room" => {
	if (activeRoomId !== roomId) return "other_room";
	return docked ? "show_docked" : "switch";
};

/** Poll `cond` until true or `timeoutMs` passes. */
export async function pollUntil(
	cond: () => boolean,
	timeoutMs: number,
	stepMs = POLL_MS,
): Promise<boolean> {
	const deadline = Date.now() + timeoutMs;
	while (!cond()) {
		if (Date.now() >= deadline) return false;
		await new Promise((r) => setTimeout(r, stepMs));
	}
	return true;
}

/** The pane API's `whenVisible`, from a "visible and laid out" predicate. */
export const makeWhenVisible =
	(isShown: () => boolean) =>
	(timeoutMs: number): Promise<boolean> =>
		pollUntil(isShown, timeoutMs);

/** Run a reveal; resolves to the answer's `revealed` flag. */
export async function revealPane(
	pane: Pick<DesignPaneApi, "whenVisible">,
	reveal: DesignReveal,
	roomId: string,
	harnessId: string,
): Promise<boolean> {
	const decision = revealDecision(reveal.activeRoomId(), roomId, reveal.isDocked(harnessId));
	if (decision === "other_room") return false;
	if (decision === "show_docked") reveal.showDocked(roomId, harnessId);
	else reveal.switchHarness(roomId, harnessId);
	return pane.whenVisible(REVEAL_TIMEOUT_MS);
}
