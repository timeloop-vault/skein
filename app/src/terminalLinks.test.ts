import { describe, expect, it } from "vitest";
import { shouldHostOpenLink } from "./terminalLinks.ts";

describe("shouldHostOpenLink", () => {
	it("host opens: mouse tracking off, CLI doesn't own clicks", () => {
		expect(shouldHostOpenLink(false, false, true)).toBe(true);
	});

	it("host opens: mouse tracking off, CLI owns clicks (no TUI running)", () => {
		expect(shouldHostOpenLink(false, true, true)).toBe(true);
	});

	it("host opens: mouse tracking on, CLI doesn't own clicks", () => {
		expect(shouldHostOpenLink(true, false, true)).toBe(true);
	});

	it("host defers: mouse tracking on and the CLI owns clicks (#269)", () => {
		expect(shouldHostOpenLink(true, true, true)).toBe(false);
	});

	it("host opens on macOS: the CLI gets a plain click, since xterm can't forward Cmd", () => {
		expect(shouldHostOpenLink(true, true, false)).toBe(true);
	});
});
