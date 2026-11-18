import { describe, expect, it } from "vitest";
import {
	bodyToStore,
	DEFAULT_CONTEXT_NUDGE_BODY,
	effectiveBody,
	isCustomBody,
} from "./contextNudge.ts";

describe("contextNudge", () => {
	it("uses the default for null, undefined and blank", () => {
		expect(effectiveBody(null)).toBe(DEFAULT_CONTEXT_NUDGE_BODY);
		expect(effectiveBody(undefined)).toBe(DEFAULT_CONTEXT_NUDGE_BODY);
		expect(effectiveBody("  \n")).toBe(DEFAULT_CONTEXT_NUDGE_BODY);
		expect(effectiveBody("hand over")).toBe("hand over");
	});

	it("stores null for a blank edit or one equal to the default", () => {
		expect(bodyToStore("")).toBeNull();
		expect(bodyToStore("   ")).toBeNull();
		expect(bodyToStore(DEFAULT_CONTEXT_NUDGE_BODY)).toBeNull();
		expect(bodyToStore("hand over")).toBe("hand over");
	});

	it("knows when a custom body is in effect", () => {
		expect(isCustomBody(null)).toBe(false);
		expect(isCustomBody(DEFAULT_CONTEXT_NUDGE_BODY)).toBe(false);
		expect(isCustomBody("x")).toBe(true);
	});
});
