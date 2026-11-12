import { describe, expect, it } from "vitest";
import { hydrateFailure } from "./hydrateFailure.ts";

describe("hydrateFailure", () => {
	it("keeps #167: a late non-resume failure after the load succeeded is silent", () => {
		expect(hydrateFailure(new Error("x"), false, true)).toEqual({
			message: null,
			hydratedOnce: true,
		});
	});

	it("shows the card for a failed db_load_rooms", () => {
		expect(hydrateFailure(new Error("locked"), false, false)).toEqual({
			message: "locked",
			hydratedOnce: false,
		});
		expect(hydrateFailure("raw", false, false).message).toBe("raw");
	});
});
