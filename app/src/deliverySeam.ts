// `skein://delivery-seam` (#733): the daemon wrote a paste or an Enter into a
// harness's PTY. The composer-draft tracker has to hear it as it used to hear
// the in-webview send's own paste/submit — and it must not count as the user
// typing, so it goes to `noteDraftEvent` only, never `noteUserInput`.

import type { DraftEvent } from "./composerDraft.ts";

export interface DeliverySeamPayload {
	harnessId: string;
	event: "paste" | "submit";
}

/// The draft event for a seam event; null for anything unrecognised.
export function seamDraftEvent(payload: { event?: unknown }): DraftEvent | null {
	if (payload.event === "paste") return { type: "seamPaste" };
	if (payload.event === "submit") return { type: "seamSubmit" };
	return null;
}
