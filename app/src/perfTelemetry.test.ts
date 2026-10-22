import { describe, expect, it } from "vitest";
import { STALL_WARN_MS, stallWarning } from "./perfTelemetry.ts";

describe("stallWarning", () => {
	it("is silent at or below the threshold", () => {
		expect(stallWarning(0)).toBeNull();
		expect(stallWarning(STALL_WARN_MS)).toBeNull();
	});
	it("warns strictly over the threshold, rounded", () => {
		expect(stallWarning(STALL_WARN_MS + 0.6)).toBe("ui stall drift=1001ms");
		expect(stallWarning(1234)).toBe("ui stall drift=1234ms");
	});
});
