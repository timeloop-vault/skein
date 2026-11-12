import { describe, expect, it } from "vitest";
import { hydrateFailure, ResumePrepareError } from "./hydrateFailure.ts";

describe("hydrateFailure", () => {
	const resume = new ResumePrepareError(new Error("daemon down"));

	it("shows the card for a failed resume call and releases the guard", () => {
		const f = hydrateFailure(resume, false, true);
		expect(f.message).toBe("Couldn't prepare the saved harnesses to resume: daemon down");
		expect(f.hydratedOnce).toBe(false);
	});

	it("stays silent once another hydrate has loaded the rooms", () => {
		expect(hydrateFailure(resume, true, true)).toEqual({ message: null, hydratedOnce: true });
	});

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
