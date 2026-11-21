import { describe, expect, it } from "vitest";
import {
	type BufferLike,
	type CellLike,
	MAX_JOINED_ROWS,
	MAX_PATHS,
	pathLinks,
	type Row,
	rowsAround,
} from "./terminalPathText.ts";

// One buffer row per string, never soft-wrapped: every break is a hard one.
function rows(...lines: string[]): Row[] {
	return lines.map((text, r) => ({
		text,
		cells: [...text].map((_, x) => ({ x: x + 1, y: r + 1 })),
		top: r + 1,
		bottom: r + 1,
	}));
}
/** Each link's readings, longest first. */
const readings = (lines: string[], focus = 0) =>
	pathLinks(rows(...lines), focus).map((link) => link.map((v) => v.path));
/** Each link's longest reading, as the text it underlines. */
const longest = (line: string) => pathLinks(rows(line), 0).map((link) => link[0]?.text);

describe("pathLinks on one row", () => {
	it("finds the absolute path an agent prints on its own line", () => {
		const line = "   /private/tmp/claude-502/scratchpad/main-sheet-light-c.png";
		expect(pathLinks(rows(line), 0)).toEqual([
			[
				{
					path: "/private/tmp/claude-502/scratchpad/main-sheet-light-c.png",
					text: "/private/tmp/claude-502/scratchpad/main-sheet-light-c.png",
					start: { x: 4, y: 1 },
					end: { x: line.length, y: 1 },
				},
			],
		]);
	});

	it("takes home, Windows and relative forms", () => {
		expect(readings(["see ~/notes/a.md and C:\\Users\\x\\shot.png or D:/y.txt"])).toEqual([
			["~/notes/a.md"],
			["C:\\Users\\x\\shot.png"],
			["D:/y.txt"],
		]);
		expect(readings(["edited app/src/foo.ts, ./run.sh and ../up/x"])).toEqual([
			["app/src/foo.ts"],
			["./run.sh"],
			["../up/x"],
		]);
	});

	it("keeps a :line:col suffix in the link but not in the path", () => {
		const [link] = pathLinks(rows("error at src/lib.rs:42:7: boom"), 0);
		expect(link?.[0]).toMatchObject({
			path: "src/lib.rs",
			text: "src/lib.rs:42:7",
			start: { x: 10, y: 1 },
			end: { x: 24, y: 1 },
		});
	});

	it("drops surrounding brackets, a leading @ and trailing punctuation", () => {
		expect(longest("(see /tmp/a.png). Also [@app/b.ts], /tmp/c.png:")).toEqual([
			"/tmp/a.png",
			"app/b.ts",
			"/tmp/c.png",
		]);
	});

	it("reads the path inside a tool call, after the whole token", () => {
		const line = "⏺ Read(/private/tmp/scratchpad/n17.png)";
		const [link] = pathLinks(rows(line), 0);
		expect(link?.map((v) => v.path)).toEqual([
			"Read(/private/tmp/scratchpad/n17.png",
			"/private/tmp/scratchpad/n17.png",
		]);
		expect(link?.[1]).toMatchObject({
			text: "/private/tmp/scratchpad/n17.png",
			start: { x: 8, y: 1 },
			end: { x: line.length - 1, y: 1 },
		});
		expect(readings(["Update(src/lib.rs:42)"])).toEqual([["Update(src/lib.rs", "src/lib.rs"]]);
	});

	it("keeps parentheses that belong to the path", () => {
		expect(readings(["app/(group)/page.tsx"])).toEqual([["app/(group)/page.tsx"]]);
		expect(readings(["Read(app/(group)/page.tsx)"])).toEqual([
			["Read(app/(group)/page.tsx", "app/(group)/page.tsx"],
		]);
	});

	it("stops at quotes and backticks", () => {
		expect(longest(`open "/tmp/a b.png" or \`/tmp/c.png\``)).toEqual(["/tmp/a", "/tmp/c.png"]);
	});

	it("leaves URLs, globs, comments and lone separators alone", () => {
		expect(readings(["https://example.com/a/b file:///tmp/x"])).toEqual([]);
		expect(readings(["src/**/*.ts // note / and . or ./ ../ ~/"])).toEqual([]);
		expect(readings(["no paths here: 12:30, e.g. v0.2.x"])).toEqual([]);
	});

	it("proposes prose with a slash, which the stat then rejects", () => {
		expect(readings(["read and/or write"])).toEqual([["and/or"]]);
	});

	it("caps the readings it asks about", () => {
		const line = Array.from({ length: MAX_PATHS + 5 }, (_, i) => `/p/${i}`).join(" ");
		expect(pathLinks(rows(line), 0).flat()).toHaveLength(MAX_PATHS);
	});
});

describe("pathLinks across a hard break", () => {
	// What Claude Code prints for a path wider than its column: the rest on
	// the next row, re-indented.
	const shot = [
		"- Main, after the switch:",
		"  /private/tmp/claude-502/scr",
		"  atchpad/a.png",
		"- Next:",
	];

	it("reads the two halves joined first, then the first alone", () => {
		// The heading above is shorter than the first half, so it can't
		// be what the path continues; `- Next:` can't be ruled out.
		expect(readings(shot, 1)).toEqual([
			[
				"/private/tmp/claude-502/scratchpad/a.png-",
				"/private/tmp/claude-502/scratchpad/a.png",
				"/private/tmp/claude-502/scr",
			],
		]);
	});

	it("reads a tool call's path broken across rows", () => {
		expect(readings(["⏺ Read(/private/tmp/claude-502/scratch", "  pad/n17.png)"])[0]).toEqual([
			"Read(/private/tmp/claude-502/scratchpad/n17.png",
			"/private/tmp/claude-502/scratchpad/n17.png",
			"Read(/private/tmp/claude-502/scratch",
			"/private/tmp/claude-502/scratch",
		]);
	});

	it("underlines from the first half's start to the second half's end", () => {
		const joined = pathLinks(rows(...shot), 1)[0]?.find(
			(v) => v.path === "/private/tmp/claude-502/scratchpad/a.png",
		);
		expect(joined).toMatchObject({ start: { x: 3, y: 2 }, end: { x: 15, y: 3 } });
	});

	it("finds the same reading from the second half", () => {
		const paths = readings(shot, 2).flat();
		expect(paths).toContain("/private/tmp/claude-502/scratchpad/a.png");
	});

	it("chains through rows the path fills on its own", () => {
		const paths = readings(["/very/long/pa", "th/middle/pa", "rt/end.png"], 1).flat();
		expect(paths[0]).toBe("/very/long/path/middle/part/end.png");
	});

	it("joins only a row's first and last tokens, and stops at a busy row", () => {
		expect(readings(["x /a/b /c/d", "e.png and", "/f/g.png"], 0)).toEqual([
			["/a/b"],
			["/c/de.png", "/c/d"],
		]);
	});

	it("never continues a row with a longer one", () => {
		expect(readings(["src/", "target/"], 0)).toEqual([["src/"]]);
		expect(readings(["src/", "target/"], 1)).toEqual([["target/"]]);
		// A second half that isn't path-shaped alone still joins.
		expect(readings(["/a/b/c", "d.png"], 1)).toEqual([["/a/b/cd.png"]]);
	});

	it("never reaches further than MAX_JOINED_ROWS", () => {
		const lines = Array.from({ length: MAX_JOINED_ROWS + 3 }, (_, i) => `/p${i}`);
		const [link] = pathLinks(rows(...lines), 0);
		expect(link?.[0]?.path).toBe(lines.slice(0, MAX_JOINED_ROWS + 1).join(""));
	});
});

// A buffer of fixed-width rows; "\0" is the trailing half of a wide char.
function fakeBuffer(lines: { text: string[]; wrapped: boolean }[]): BufferLike {
	const cell = (chars: string, width: number): CellLike => ({
		getChars: () => chars,
		getWidth: () => width,
	});
	return {
		getLine: (y) => {
			const line = lines[y];
			if (!line) return undefined;
			return {
				isWrapped: line.wrapped,
				length: line.text.length,
				getCell: (x) => {
					const ch = line.text[x] ?? "";
					return ch === "\0" ? cell("", 0) : cell(ch, 1);
				},
			};
		},
	};
}
const line = (s: string, wrapped = false) => ({ text: [...s], wrapped });

describe("rowsAround", () => {
	it("joins soft-wrapped rows and maps every char to its cell", () => {
		const buf = fakeBuffer([
			line("other"),
			line("ab /tm"),
			line("p/x.pn", true),
			line("g end", true),
			line("next"),
		]);
		const { rows: around, focus } = rowsAround(buf, 3);
		expect(around.map((r) => r.text)).toEqual(["other", "ab /tmp/x.png end", "next"]);
		expect(focus).toBe(1);
		const [link] = pathLinks(around, focus);
		expect(link?.[0]).toMatchObject({
			path: "/tmp/x.png",
			start: { x: 4, y: 2 },
			end: { x: 1, y: 4 },
		});
	});

	it("reads empty cells as spaces and skips a wide char's second half", () => {
		const buf = fakeBuffer([{ text: ["界", "\0", "/", "a", "/", "b", "", ""], wrapped: false }]);
		const [row] = rowsAround(buf, 1).rows;
		expect(row?.text).toBe("界/a/b  ");
		expect(row?.cells[1]).toEqual({ x: 3, y: 1 });
	});

	it("takes up to MAX_JOINED_ROWS logical lines either side, within the buffer", () => {
		const buf = fakeBuffer(Array.from({ length: 20 }, (_, i) => line(`r${i}`)));
		const mid = rowsAround(buf, 10);
		expect(mid.rows).toHaveLength(2 * MAX_JOINED_ROWS + 1);
		expect(mid.rows[mid.focus]?.text).toBe("r9");
		const first = rowsAround(buf, 1);
		expect(first.focus).toBe(0);
		expect(first.rows.map((r) => r.text)).toEqual(["r0", "r1", "r2", "r3", "r4"]);
		const last = rowsAround(buf, 20);
		expect(last.rows.map((r) => r.text)).toEqual(["r15", "r16", "r17", "r18", "r19"]);
	});
});
