// The review header's Nudge button (#238). Target is always the room's
// active harness — no picker — and which prompt applies is a pure
// function of the review's own state (sign-off, then unresolved
// threads). `useSendGateHint` keeps enablement live as the harness's
// phase moves, asking the daemon's gate. Split out of
// ReviewPane.tsx (#460).

import { HARNESS_KINDS } from "../data.tsx";
import { sendPrompt } from "../harnessInput.ts";
import { useNudgeOverrides } from "../nudgeStore.ts";
import type { Harness } from "../types.ts";
import { useSendGateHint } from "../useSendGateHint.ts";
import { selectNudge } from "./nudges.ts";
import { type SignoffStatus, signoffState } from "./signoff.ts";

export const useReviewNudge = (
	activeHarness: Harness | undefined,
	signoff: SignoffStatus | undefined,
	headSha: string | undefined,
	unresolvedCount: number,
	setActionError: (message: string | undefined) => void,
) => {
	const activeCapabilities = activeHarness ? HARNESS_KINDS[activeHarness.kind].capabilities : null;
	const nudgeOverrides = useNudgeOverrides();
	const nudge = selectNudge(signoffState(signoff, headSha), unresolvedCount, nudgeOverrides);
	// A display hint for the disabled button only; the daemon decides (#733).
	const sendHint = useSendGateHint(activeHarness?.id ?? null, Boolean(activeHarness && nudge));
	const nudgeGate = activeHarness && nudge ? sendHint(nudge.body) : undefined;
	const nudgeDisabledReason = !nudge
		? "Nothing to nudge about"
		: nudgeGate && !nudgeGate.ok
			? nudgeGate.reason
			: undefined;
	const onNudge = () => {
		if (!activeHarness || !nudge) return;
		void sendPrompt(activeHarness.id, nudge.body).then((result) => {
			if (!result.ok) setActionError(result.reason);
		});
	};
	return { activeCapabilities, nudge, nudgeGate, nudgeDisabledReason, onNudge };
};
