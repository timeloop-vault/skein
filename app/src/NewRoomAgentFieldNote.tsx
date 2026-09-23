import type { AgentListing } from "./agents.ts";
import { NO_REVIEW_TOOLS_TITLE } from "./components.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import type { HarnessKind } from "./types.ts";

// Split out of `NewRoomDialog.tsx` (#19) — the one line under the Agent
// field in the new-room dialog.

/** The one line under the Agent field. At most one thing is worth
 *  saying at a time, and the order is the order of consequence:
 *  a name that is gone blocks the create; an agent that cannot see the
 *  review tools silently switches off the #52 loop; a degraded list
 *  means the field is a guess. Nothing to say = no line, because a
 *  permanent note under a field stops being read. */
export const AgentFieldNote = ({
	agent,
	listing,
	missing,
	kind,
}: {
	agent: string | undefined;
	listing: AgentListing | null;
	missing: boolean;
	kind: HarnessKind;
}) => {
	const note = (() => {
		if (missing && agent) {
			return {
				cls: "err" as const,
				text: `${HARNESS_KINDS[kind].name} does not offer "${agent}" any more — pick another.`,
			};
		}
		const picked = agent ? listing?.agents.find((a) => a.name === agent) : undefined;
		if (picked && !picked.allowsReviewTools) {
			return { cls: "warn" as const, text: NO_REVIEW_TOOLS_TITLE };
		}
		if (listing?.degraded) {
			return {
				cls: "warn" as const,
				text: `This list may be incomplete — ${listing.degraded}`,
			};
		}
		return null;
	})();
	if (!note) return null;
	return (
		<div
			style={{
				fontFamily: "var(--sk-mono)",
				fontSize: 10.5,
				marginTop: 4,
				lineHeight: 1.5,
				color: note.cls === "err" ? "var(--err)" : "var(--warn)",
			}}
		>
			{note.text}
		</div>
	);
};
