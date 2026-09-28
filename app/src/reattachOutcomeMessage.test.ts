import { describe, expect, it } from "vitest";
import { reattachOutcomeMessage } from "./reattachOutcomeMessage.ts";

describe("reattachOutcomeMessage", () => {
	it("reattached: the tail was dead and is live again", () => {
		expect(reattachOutcomeMessage({ ok: true, outcome: "reattached" })).toBe(
			"Telemetry reattached; status will update from the transcript.",
		);
	});

	it("healthy: nothing to do", () => {
		expect(reattachOutcomeMessage({ ok: true, outcome: "healthy" })).toBe(
			"Telemetry is healthy; nothing to reattach.",
		);
	});

	it("not_attached: points at a restart, the only way back in", () => {
		expect(reattachOutcomeMessage({ ok: true, outcome: "not_attached" })).toBe(
			"No telemetry tail for this harness; restart the harness to reattach.",
		);
	});

	it("error: carries the underlying message through", () => {
		expect(reattachOutcomeMessage({ ok: false, error: "not found" })).toBe(
			"Reattach failed: not found",
		);
	});
});
