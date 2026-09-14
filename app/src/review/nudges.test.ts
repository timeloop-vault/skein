import { describe, expect, it } from "vitest";
import { NUDGE_ADDRESS_COMMENTS, NUDGE_LAND, NUDGE_LAPSED, selectNudge } from "./nudges.ts";

describe("selectNudge", () => {
	it("nudges to land when signed off and not stale", () => {
		expect(selectNudge("approved", 0)).toEqual({ label: "Nudge: land", body: NUDGE_LAND });
	});

	it("nudges about the lapse even with open threads", () => {
		// A lapsed sign-off is the more urgent fact — it says the agent
		// may still believe it is cleared to land.
		expect(selectNudge("stale", 3)).toEqual({ label: "Nudge: lapsed", body: NUDGE_LAPSED });
	});

	it("nudges to address comments when nothing is signed off but threads are open", () => {
		expect(selectNudge("none", 2)).toEqual({
			label: "Nudge: address comments",
			body: NUDGE_ADDRESS_COMMENTS,
		});
	});

	it("has nothing to nudge about otherwise", () => {
		expect(selectNudge("none", 0)).toBeUndefined();
	});
});
