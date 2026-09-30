import { describe, expect, it } from "vitest";
import { TRANSITION_SOURCE, stillRunningSummary } from "./harnessActivity.ts";
import { waitingNote } from "./harnessActivityLabels.ts";

describe("stillRunningSummary", () => {
	it("is null for zero or less", () => {
		expect(stillRunningSummary(0)).toBeNull();
		expect(stillRunningSummary(-1)).toBeNull();
	});

	it("is singular for one, plural otherwise", () => {
		expect(stillRunningSummary(1)).toBe("1 background task still running");
		expect(stillRunningSummary(2)).toBe("2 background tasks still running");
	});
});

describe("waitingNote", () => {
	it.each([
		[TRANSITION_SOURCE.DelegationCeiling, 3, 2, "2 background tasks still running"],
		[TRANSITION_SOURCE.WorkWatchdog, 3, 1, "1 background task still running"],
		[TRANSITION_SOURCE.WorkWatchdog, 3, 0, null],
		[TRANSITION_SOURCE.DelegationSettled, 3, 2, "3 delegated agents finished"],
		[TRANSITION_SOURCE.DelegationSettled, 0, 2, null],
	])("%s delegated=%i overdue=%i", (source, delegated, overdue, expected) => {
		expect(waitingNote(source, delegated, overdue)).toBe(expected);
	});
});
