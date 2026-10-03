import { describe, expect, it, vi } from "vitest";
import { createShellClaimTracker } from "./opencodeShellClaim.ts";
import type { OpencodeScan } from "./opencodeShellFollow.ts";

const scan = (over: Partial<OpencodeScan> = {}): OpencodeScan => ({
	pid: 10,
	sessionId: "ses_a",
	port: null,
	portConfirmed: false,
	continueLast: false,
	...over,
});

const setup = () => {
	const claim = { set: vi.fn(), release: vi.fn() };
	const followed = vi.fn();
	return { claim, followed, t: createShellClaimTracker({ claim, followed }) };
};

describe("createShellClaimTracker", () => {
	it("claims a proven argv session without a port", () => {
		const { t, claim, followed } = setup();
		t.observe(scan(), "ses_a");
		expect(claim.set).toHaveBeenCalledWith("ses_a", undefined);
		expect(followed).not.toHaveBeenCalled();
	});

	it("claims with the port when it is confirmed", () => {
		const { t, claim } = setup();
		t.observe(scan({ port: 4000, portConfirmed: true }), "ses_a");
		expect(claim.set).toHaveBeenCalledWith("ses_a", 4000);
	});

	it("an unconfirmed port is not claimed", () => {
		const { t, claim } = setup();
		t.observe(scan({ port: 4000 }), "ses_a");
		expect(claim.set).toHaveBeenCalledWith("ses_a", undefined);
	});

	it("updates the claim when the port is confirmed later", () => {
		const { t, claim } = setup();
		t.observe(scan({ port: 4000 }), "ses_a");
		t.observe(scan({ port: 4000, portConfirmed: true }), undefined);
		expect(claim.set).toHaveBeenCalledTimes(2);
		expect(claim.set).toHaveBeenLastCalledWith("ses_a", 4000);
		t.observe(scan({ port: 4000, portConfirmed: true }), undefined);
		expect(claim.set).toHaveBeenCalledTimes(2);
	});

	it("claims an SSE-followed session during the proven window, keeping the port", () => {
		const { t, claim, followed } = setup();
		t.observe(scan({ sessionId: null, port: 4000, portConfirmed: true }), undefined);
		expect(claim.set).not.toHaveBeenCalled();
		t.followed("ses_new");
		expect(claim.set).toHaveBeenCalledWith("ses_new", 4000);
		expect(followed).not.toHaveBeenCalled();
	});

	it("an SSE follow outside the proven window is a plain session replace", () => {
		const { t, claim, followed } = setup();
		t.followed("ses_x");
		expect(followed).toHaveBeenCalledWith("ses_x");
		expect(claim.set).not.toHaveBeenCalled();
	});

	it("releases when the scan finds nothing after a claim", () => {
		const { t, claim, followed } = setup();
		t.observe(scan(), "ses_a");
		t.gone();
		expect(claim.release).not.toHaveBeenCalled();
		t.gone();
		expect(claim.release).toHaveBeenCalledTimes(1);
		t.gone();
		expect(claim.release).toHaveBeenCalledTimes(1);
		t.followed("ses_z");
		expect(followed).toHaveBeenCalledWith("ses_z");
	});

	it("releases when a different pid shows up, even an unproven one", () => {
		const { t, claim } = setup();
		t.observe(scan(), "ses_a");
		t.seen(10);
		expect(claim.release).not.toHaveBeenCalled();
		t.seen(11);
		expect(claim.release).toHaveBeenCalledTimes(1);
	});

	it("never claims or releases for an unproven process", () => {
		const { t, claim } = setup();
		t.seen(10);
		t.gone();
		expect(claim.set).not.toHaveBeenCalled();
		expect(claim.release).not.toHaveBeenCalled();
	});

	it("without a sink an argv session is a plain replace", () => {
		const followed = vi.fn();
		const t = createShellClaimTracker({ claim: undefined, followed });
		t.observe(scan(), "ses_a");
		expect(followed).toHaveBeenCalledWith("ses_a");
	});

	it("claims the proven argv session even when it equals the current one", () => {
		const { t, claim } = setup();
		t.observe(scan({ sessionId: "ses_own" }), undefined);
		expect(claim.set).toHaveBeenCalledWith("ses_own", undefined);
	});

	it("one transient empty scan then the same pid keeps the claim", () => {
		const { t, claim } = setup();
		t.observe(scan(), undefined);
		t.gone();
		t.seen(10);
		t.gone();
		expect(claim.release).not.toHaveBeenCalled();
	});

	it("re-claims the remembered session (not argv) when the same pid returns after release", () => {
		const { t, claim } = setup();
		t.observe(scan({ port: 4000, portConfirmed: true }), undefined);
		t.followed("ses_new");
		t.gone();
		t.gone();
		expect(claim.release).toHaveBeenCalledTimes(1);
		t.observe(scan({ port: 4000, portConfirmed: true }), undefined);
		expect(claim.set).toHaveBeenLastCalledWith("ses_new", 4000);
	});

	it("a --port-only process: SSE follow claims with the port", () => {
		const { t, claim } = setup();
		t.observe(scan({ sessionId: null, port: 4000, portConfirmed: true }), undefined);
		t.followed("ses_sse");
		expect(claim.set).toHaveBeenCalledWith("ses_sse", 4000);
	});
});
