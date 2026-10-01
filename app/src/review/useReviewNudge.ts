// The review header's Nudge button (#238). Target is always the room's
// active harness — no picker — and which prompt applies is a pure
// function of the review's own state (sign-off, then unresolved
// threads). `useHarnessActivity` keeps enablement live as the harness's
// phase moves, without the pane polling anything itself. Split out of
// ReviewPane.tsx (#460).

import { HARNESS_KINDS } from "../data.tsx";
import { useHarnessActivity } from "../harnessActivity.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "../harnessInput.ts";
import { useNudgeOverrides } from "../nudgeStore.ts";
import type { Harness } from "../types.ts";
import { selectNudge } from "./nudges.ts";
import { type SignoffStatus, signoffState } from "./signoff.ts";

export const useReviewNudge = (
	activeHarness: Harness | undefined,
	signoff: SignoffStatus | undefined,
	unresolvedCount: number,
	setActionError: (message: string | undefined) => void,
) => {
	const activeCapabilities = activeHarness ? HARNESS_KINDS[activeHarness.kind].capabilities : null;
	const activeActivity = useHarnessActivity(activeHarness?.id ?? null);
	const nudgeOverrides = useNudgeOverrides();
	const nudge = selectNudge(signoffState(signoff), unresolvedCount, nudgeOverrides);
	const nudgeGate =
		activeHarness && nudge
			? canSendPrompt({
					capabilities: HARNESS_KINDS[activeHarness.kind].capabilities,
					activity: activeActivity,
					registered: harnessInput.isRegistered(activeHarness.id),
					bracketedPasteOn: harnessInput.bracketedPaste(activeHarness.id),
					body: nudge.body,
				})
			: undefined;
	const nudgeDisabledReason = !nudge
		? "Nothing to nudge about"
		: nudgeGate && !nudgeGate.ok
			? nudgeGate.reason
			: undefined;
	const onNudge = () => {
		if (!activeHarness || !nudge) return;
		const result = sendPrompt(activeHarness.id, activeHarness.kind, nudge.body);
		if (!result.ok) setActionError(result.reason);
	};
	return { activeCapabilities, nudge, nudgeGate, nudgeDisabledReason, onNudge };
};
