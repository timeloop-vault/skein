// selectNudge's override-resolving arm (#355) — kept out of
// nudges.test.ts, which stays byte-identical to its pre-#355 shape as
// the acceptance check for this change.

import { describe, expect, it } from "vitest";
import type { NudgeOverrides } from "../nudgeRegistry.ts";
import { selectNudge } from "./nudges.ts";

describe("selectNudge with overrides", () => {
	it("pastes the overridden body, not the shipped-in-code default", () => {
		const overrides: NudgeOverrides = { "review-land": "Custom land instructions." };
		expect(selectNudge("approved", 0, overrides)).toEqual({
			label: "Nudge: land",
			body: "Custom land instructions.",
		});
	});

	it("falls back to the default body when there is no override for the selected nudge", () => {
		const overrides: NudgeOverrides = { "review-lapsed": "Custom lapsed instructions." };
		expect(selectNudge("approved", 0, overrides)?.body).toBe(
			"The review is signed off. Land this branch per this repo's conventions.",
		);
	});

	it("resolves the lapsed and address-comments nudges the same way", () => {
		expect(selectNudge("stale", 3, { "review-lapsed": "Lapsed override" })).toEqual({
			label: "Nudge: lapsed",
			body: "Lapsed override",
		});
		expect(selectNudge("none", 2, { "review-address-comments": "Address override" })).toEqual({
			label: "Nudge: address comments",
			body: "Address override",
		});
	});
});
