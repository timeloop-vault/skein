import { describe, expect, it } from "vitest";
import { CLEAN_DRAFT, reduceDraft } from "./composerDraft.ts";
import { seamDraftEvent } from "./deliverySeam.ts";

describe("seamDraftEvent", () => {
	it("maps paste and submit to the draft tracker's seam events", () => {
		expect(seamDraftEvent({ event: "paste" })).toEqual({ type: "seamPaste" });
		expect(seamDraftEvent({ event: "submit" })).toEqual({ type: "seamSubmit" });
	});

	it("ignores anything else", () => {
		expect(seamDraftEvent({ event: "retry" })).toBeNull();
		expect(seamDraftEvent({})).toBeNull();
	});

	it("a paste makes the draft unknown and the submit clears it, as the old in-webview send did", () => {
		const pasted = reduceDraft(
			CLEAN_DRAFT,
			seamDraftEvent({ event: "paste" }) ?? { type: "respawn" },
		);
		expect(pasted.kind).toBe("unknown");
		const submitted = reduceDraft(
			pasted,
			seamDraftEvent({ event: "submit" }) ?? { type: "respawn" },
		);
		expect(submitted.kind).toBe("clean");
	});
});
