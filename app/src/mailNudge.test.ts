import { describe, expect, it } from "vitest";
import { mailPopoverText } from "./mailNudge.ts";

describe("mailPopoverText", () => {
	it("uses singular wording for one message, no room names", () => {
		expect(mailPopoverText(1, [])).toBe("1 unread message");
	});

	it("uses plural wording for more than one message", () => {
		expect(mailPopoverText(3, [])).toBe("3 unread messages");
	});

	it("names a single room", () => {
		expect(mailPopoverText(1, ["test-5"])).toBe("1 unread message from test-5");
	});

	it("joins multiple room names with a comma", () => {
		expect(mailPopoverText(2, ["Alpha", "Beta"])).toBe("2 unread messages from Alpha, Beta");
	});
});
