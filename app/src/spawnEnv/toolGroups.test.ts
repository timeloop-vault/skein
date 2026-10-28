import { describe, expect, it } from "vitest";
import type { ToolGroups } from "../types.ts";
import { groupEnabled, TOOL_GROUPS, withGroup } from "./toolGroups.ts";

describe("toolGroups", () => {
	it("lists the five groups in order", () => {
		expect(TOOL_GROUPS.map((g) => g.key)).toEqual([
			"review",
			"mail",
			"rooms",
			"directory",
			"design",
		]);
	});

	it("treats a missing toolGroups object as on", () => {
		expect(groupEnabled({}, "mail")).toBe(true);
	});

	it("treats a missing key as on and honours an explicit false", () => {
		const s = { toolGroups: { rooms: false } };
		expect(groupEnabled(s, "review")).toBe(true);
		expect(groupEnabled(s, "rooms")).toBe(false);
	});

	it("withGroup returns a copy and leaves the input alone", () => {
		const s = { toolGroups: { mail: true } };
		const next = withGroup(s, "mail", false);
		expect(next).not.toBe(s);
		expect(next.toolGroups).not.toBe(s.toolGroups);
		expect(s.toolGroups).toEqual({ mail: true });
		expect(groupEnabled(next, "mail")).toBe(false);
		expect(withGroup<{ toolGroups?: Partial<ToolGroups> }>({}, "design", false).toolGroups).toEqual(
			{ design: false },
		);
	});
});
