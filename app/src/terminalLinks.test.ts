import { describe, expect, it } from "vitest";
import { HARNESS_KINDS } from "./data.tsx";
import { isOwnSchemeLink, shouldHostOpenLink, URI_REGEX, uriScheme } from "./terminalLinks.ts";

/** Every link WebLinkProvider would find in `line`: same `g` it appends. */
function links(line: string): string[] {
	const rex = new RegExp(URI_REGEX.source, `${URI_REGEX.flags}g`);
	return [...line.matchAll(rex)].map((m) => m[0]);
}

describe("URI_REGEX", () => {
	it.each([
		["see https://github.com/o/r/pull/12.", "https://github.com/o/r/pull/12"],
		["https://x.dev/a, and more", "https://x.dev/a"],
		["at https://x.dev/a; then", "https://x.dev/a"],
		["open https://x.dev/a: done", "https://x.dev/a"],
		["try https://x.dev/a!", "https://x.dev/a"],
		["really https://x.dev/a?", "https://x.dev/a"],
		["ends https://x.dev/a...", "https://x.dev/a"],
		["(https://x.dev/a).", "https://x.dev/a"],
		['"https://x.dev/a",', "https://x.dev/a"],
	])("drops trailing punctuation: %s", (line, url) => {
		expect(links(line)).toEqual([url]);
	});

	it.each([
		"https://x.dev/a.html",
		"https://x.dev/search?q=1&r=2",
		"https://x.dev:8080/a",
		"https://x.dev/a#frag",
		"https://x.dev/a/",
		"vscode://file/Users/me/a.ts:12:3",
		"file:///tmp/a.txt",
	])("keeps a URL whole: %s", (url) => {
		expect(links(`go ${url} now`)).toEqual([url]);
	});

	it("finds two links on one line", () => {
		expect(links("a https://x.dev/1, b https://y.dev/2.")).toEqual([
			"https://x.dev/1",
			"https://y.dev/2",
		]);
	});

	it("ignores scope operators", () => {
		expect(links("std::vector and a::b")).toEqual([]);
	});
});

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
				ownScheme: false,
				modifierHeld: true,
			}),
		).toBe(hostOpens);
	});

	const base = {
		isMac: false,
		mouseClicksDisabled: false,
		mouseTrackingOn: true,
		cliOpensClickedLinks: kinds.claude,
		ownScheme: true,
		modifierHeld: true,
	};

	it.each([
		[false, true],
		[false, false],
		[true, true],
		[true, false],
	])("own scheme + modifier opens (mac %s, tracking %s)", (isMac, mouseTrackingOn) => {
		expect(shouldHostOpenLink({ ...base, isMac, mouseTrackingOn })).toBe(true);
	});

	it("own scheme, plain click: opens only with tracking on", () => {
		expect(shouldHostOpenLink({ ...base, modifierHeld: false })).toBe(true);
		expect(shouldHostOpenLink({ ...base, modifierHeld: false, mouseTrackingOn: false })).toBe(
			false,
		);
	});

	it("foreign scheme, plain click: never opens", () => {
		for (const mouseTrackingOn of [false, true]) {
			for (const isMac of [false, true]) {
				expect(
					shouldHostOpenLink({
						...base,
						ownScheme: false,
						modifierHeld: false,
						mouseTrackingOn,
						isMac,
					}),
				).toBe(false);
			}
		}
	});

	it("foreign scheme + modifier, tracking on, claude, win: defers", () => {
		expect(shouldHostOpenLink({ ...base, ownScheme: false })).toBe(false);
	});
});

describe("uriScheme / isOwnSchemeLink", () => {
	it("extracts a lowercased scheme", () => {
		expect(uriScheme("Skein-Dev://room/x")).toBe("skein-dev");
		expect(uriScheme("no scheme")).toBeNull();
		expect(uriScheme("://x")).toBeNull();
	});

	it("matches only the configured scheme, case-insensitively", () => {
		expect(isOwnSchemeLink("skein-dev://room/x", ["skein-dev"])).toBe(true);
		expect(isOwnSchemeLink("SKEIN-DEV://room/x", ["skein-dev"])).toBe(true);
		expect(isOwnSchemeLink("skein://room/x", ["skein-dev"])).toBe(false);
		expect(isOwnSchemeLink("https://x.dev", ["skein-dev"])).toBe(false);
		expect(isOwnSchemeLink("skein://room/x", [])).toBe(false);
	});
});
