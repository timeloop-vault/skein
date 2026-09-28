// The held-mail badge's "deliver now" click (#413). A separate module so
// `mailHold.ts` (a leaf store) need not import `harnessInput.ts`.
//
// The click skips the draft guard, but a nudge pasted at the cursor inside
// a real user draft would submit both, so the click first takes a fresh
// screen reading (no settle window: the user is clicking, not typing).

import type { ComposerDraft } from "./composerDraft.ts";
import { confirmDialog } from "./confirmDialog.ts";
import { logBoth } from "./frontendLog.ts";
import { harnessInput } from "./harnessInput.ts";
import { mailHold } from "./mailHold.ts";
import type { ComposerReading } from "./promptScreen.ts";

export const DELIVER_NOW_REFUSAL = "Clear or send your draft first";

/// Text wins over everything: a "text" reading, or keystrokes that say
/// "typed", refuse even against an "empty" reading. Only a confident
/// "empty" delivers; "unknown" or no reading asks the user.
export function decideRelease(
	draft: ComposerDraft,
	reading: ComposerReading | null,
): "deliver" | "refuse" | "confirm" {
	if (reading === "text" || draft.kind === "typed") return "refuse";
	if (reading === "empty") return "deliver";
	return "confirm";
}

export async function requestDeliverNow(harnessId: string): Promise<void> {
	const decision = decideRelease(
		harnessInput.draft(harnessId),
		harnessInput.readComposerNow(harnessId),
	);
	if (decision === "refuse") {
		mailHold.setReleaseRefusal(harnessId, DELIVER_NOW_REFUSAL);
		logBoth(
			"info",
			"skein::seam",
			`[skein] deliver-now refused — composer shows a draft (#413) (harness ${harnessId})`,
		);
		return;
	}
	if (decision === "confirm") {
		let ok = false;
		try {
			ok = await confirmDialog({
				title: "Deliver held mail?",
				message: "The prompt may hold a draft; deliver anyway?",
				confirmLabel: "Deliver",
				kind: "warning",
			});
		} catch (err) {
			console.error("[skein] confirmDialog failed:", err);
		}
		if (!ok) return;
	}
	mailHold.release(harnessId);
}
