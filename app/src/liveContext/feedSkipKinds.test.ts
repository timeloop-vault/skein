import { describe, expect, it } from "vitest";
import { SKIP_KINDS } from "./feedItemsHelpers.ts";

describe("SKIP_KINDS", () => {
	it("keeps cost-only rows out of the feed", () => {
		expect(SKIP_KINDS.has("api_usage")).toBe(true);
		expect(SKIP_KINDS.has("cost_state")).toBe(true);
		expect(SKIP_KINDS.has("tool_call")).toBe(false);
	});
});
