import { describe, expect, it } from "vitest";
import {
	type SignoffStatus,
	approveConfirmation,
	short,
	signoffState,
	staleExplanation,
} from "./signoff.ts";

const status = (over: Partial<SignoffStatus> = {}): SignoffStatus => ({
	approved: false,
	stale: false,
	unresolvedCount: 0,
	unaddressedCount: 0,
	canSignOff: true,
	...over,
});

describe("signoffState", () => {
	it("keeps approved and lapsed as different answers", () => {
		// The whole feature is that an approval names a commit. Folding
		// "approved" and "approved, but not of this code" into one state
		// would put the lie back in.
		expect(signoffState(status())).toBe("none");
		expect(signoffState(status({ approved: true }))).toBe("approved");
		expect(signoffState(status({ stale: true }))).toBe("stale");
	});

	it("reads a missing status as unapproved rather than throwing", () => {
		// The control renders before the first fetch returns.
		expect(signoffState(undefined)).toBe("none");
	});
});

describe("approveConfirmation", () => {
	it("names the commit being approved", () => {
		const text = approveConfirmation(status({ headSha: "abcdef1234567890" }));
		expect(text).toContain("abcdef12");
		expect(text).toContain("lapses");
	});

	it("falls back to the branch when there is no head sha", () => {
		expect(approveConfirmation(status())).toContain("this branch");
	});

	it("says how many threads are still open, and that approving leaves them open", () => {
		// Open threads never block a sign-off — that is the reviewer's
		// call — but they should not be able to approve without being
		// told what they are approving over.
		const one = approveConfirmation(status({ unresolvedCount: 1 }));
		expect(one).toContain("1 comment thread is still open");
		expect(one).toContain("does not close it");

		const many = approveConfirmation(status({ unresolvedCount: 3 }));
		expect(many).toContain("3 comment threads are still open");
		expect(many).toContain("does not close them");
	});

	it("says nothing about threads when there are none", () => {
		expect(approveConfirmation(status())).not.toContain("still open");
	});
});

describe("staleExplanation", () => {
	it("names the approved sha and how far HEAD has moved", () => {
		const text = staleExplanation(
			status({ stale: true, approvedSha: "abcdef1234567890", commitsSince: 2 }),
		);
		expect(text).toContain("abcdef12");
		expect(text).toContain("2 commits landed since");
	});

	it("singularises one commit", () => {
		expect(staleExplanation(status({ stale: true, commitsSince: 1 }))).toContain(
			"1 commit landed since",
		);
	});

	it("says the branch was rewritten when the distance is unknowable", () => {
		// A rebase or an amend leaves the approved sha unreachable, and
		// inventing "3 commits since" would be a fabrication.
		const text = staleExplanation(status({ stale: true, approvedSha: "abcdef1234567890" }));
		expect(text).toContain("rewritten since");
		expect(text).not.toContain("commit ");
	});
});

describe("short", () => {
	it("trims a sha to something a one-line control can hold", () => {
		expect(short("abcdef1234567890")).toBe("abcdef12");
		expect(short(undefined)).toBe("");
	});
});
