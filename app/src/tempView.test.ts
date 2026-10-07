import { describe, expect, it } from "vitest";
import { pickerAfterHarnessSelect, shownActiveId } from "./tempView.ts";

describe("shownActiveId", () => {
	it("hides the active id while a temp view is open", () => {
		expect(shownActiveId("a", true)).toBeNull();
	});
	it("passes the active id through when closed", () => {
		expect(shownActiveId("a", false)).toBe("a");
		expect(shownActiveId(null, false)).toBeNull();
	});
});

describe("pickerAfterHarnessSelect", () => {
	it("closes the picker when it is open for that room", () => {
		expect(pickerAfterHarnessSelect("r1", "r1")).toBeNull();
	});
	it("leaves a picker open for another room, or none, unchanged", () => {
		expect(pickerAfterHarnessSelect("r2", "r1")).toBe("r2");
		expect(pickerAfterHarnessSelect(null, "r1")).toBeNull();
	});
});
