import { describe, expect, it } from "vitest";
import {
	type SignoffStatus,
	approvePrompt,
	short,
	signoffState,
	staleExplanation,
	withdrawPrompt,
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

describe("approvePrompt", () => {
	it("names the commit, and says the approval is not permanent", () => {
		// Both halves matter: what is being signed off, and that a later
		// commit revokes it — a reviewer who thinks it is permanent has
		// been misled about the one rule the feature runs on.
		const p = approvePrompt(status({ headSha: "abcdef1234567890" }));
		expect(p.question).toContain("abcdef12");
		expect(p.detail).toContain("clearance to land");
		expect(p.detail).toContain("lapse");
	});

	it("falls back to the branch when there is no head sha", () => {
		expect(approvePrompt(status()).question).toContain("this branch");
	});

	it("says how many threads are still open, and that signing off leaves them open", () => {
		// Open threads never block a sign-off — that is the reviewer's
		// call — but they should not be able to approve without being
		// told what they are approving over.
		const one = approvePrompt(status({ unresolvedCount: 1 })).openThreads;
		expect(one).toContain("1 comment thread is still open");
		expect(one).toContain("does not close it");

		const many = approvePrompt(status({ unresolvedCount: 3 })).openThreads;
		expect(many).toContain("3 comment threads are still open");
		expect(many).toContain("does not close them");
	});

	it("omits the open-threads line entirely when there are none", () => {
		// `exactOptionalPropertyTypes` is on, so absent must mean absent
		// rather than an empty string the row would still render.
		expect(approvePrompt(status()).openThreads).toBeUndefined();
	});
});

describe("withdrawPrompt", () => {
	it("says what withdrawing means to the agent", () => {
		const p = withdrawPrompt();
		expect(p.question).toContain("withdraw");
		expect(p.detail).toContain("no longer cleared to land");
		expect(p.openThreads).toBeUndefined();
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
