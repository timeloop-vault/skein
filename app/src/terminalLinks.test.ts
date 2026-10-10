import { describe, expect, it } from "vitest";
import { HARNESS_KINDS } from "./data.tsx";
import { shouldHostOpenLink } from "./terminalLinks.ts";

const kinds = {
	claude: HARNESS_KINDS.claude.capabilities.opensClickedLinks,
	byoh: HARNESS_KINDS.byoh.capabilities.opensClickedLinks,
};

type Row = [string, boolean, boolean, boolean, "claude" | "byoh", boolean];

// [label, isMac, mouseClicksDisabled, mouseTrackingOn, kind, hostOpens]
const rows: Row[] = [];
for (const isMac of [false, true]) {
	for (const disabled of [false, true]) {
		for (const tracking of [false, true]) {
			for (const kind of ["claude", "byoh"] as const) {
				// Exactly one combination defers: #269 on non-mac, clicks enabled.
				const defers = !isMac && !disabled && tracking && kind === "claude";
				rows.push([
					`${isMac ? "mac" : "win/linux"}, clicks ${disabled ? "disabled" : "enabled"}, tracking ${tracking ? "on" : "off"}, ${kind}`,
					isMac,
					disabled,
					tracking,
					kind,
					!defers,
				]);
			}
		}
	}
}

describe("shouldHostOpenLink", () => {
	it("has 16 rows, exactly one deferring", () => {
		expect(rows).toHaveLength(16);
		expect(rows.filter((r) => !r[5])).toHaveLength(1);
	});

	it.each(rows)("%s", (_label, isMac, mouseClicksDisabled, mouseTrackingOn, kind, hostOpens) => {
		expect(
			shouldHostOpenLink({
				isMac,
				mouseClicksDisabled,
				mouseTrackingOn,
				cliOpensClickedLinks: kinds[kind],
			}),
		).toBe(hostOpens);
	});
});
