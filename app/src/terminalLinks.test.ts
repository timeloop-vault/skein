import { describe, expect, it } from "vitest";
import { shouldHostOpenLink } from "./terminalLinks.ts";

describe("shouldHostOpenLink", () => {
	it("host opens: mouse tracking off, CLI doesn't own clicks", () => {
		expect(shouldHostOpenLink(false, false)).toBe(true);
	});

	it("host opens: mouse tracking off, CLI owns clicks (no TUI running)", () => {
		expect(shouldHostOpenLink(false, true)).toBe(true);
	});

	it("host opens: mouse tracking on, CLI doesn't own clicks", () => {
		expect(shouldHostOpenLink(true, false)).toBe(true);
	});

	it("host defers: mouse tracking on and the CLI owns clicks (#269)", () => {
		expect(shouldHostOpenLink(true, true)).toBe(false);
	});
});
