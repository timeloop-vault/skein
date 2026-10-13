import { describe, expect, it } from "vitest";
import type { ScreenCell, ScreenSnapshot } from "./promptScreen";
import { readScreenPhase } from "./screenPhase";
import type { HarnessKind } from "./types";

const COLS = 120;
const ROWS = 40;

function screen(lines: Record<number, string>): ScreenSnapshot {
	const rows: ScreenCell[][] = [];
	for (let y = 0; y < ROWS; y++) {
		const chars = [...(lines[y] ?? "")];
		const cells: ScreenCell[] = [];
		for (let x = 0; x < COLS; x++) cells.push({ ch: chars[x] ?? " ", dim: false });
		rows.push(cells);
	}
	return { rows, cursorX: 0, cursorY: 0 };
}

const TITLE = "Example session";
const BUSY = (face: string, verb: string): string =>
	` ─ ${face} ${verb}… · 35s │ haiku 4.5 │ 13.9k/200k │ [█░░░░░░░░░] 7%                ─  ${TITLE}`;
const READY = ` ─ ready │ haiku 4.5 │ 14.4k/200k │ [█░░░░░░░░░] 7% │ 3m 22s │ ✓ 56s │ ◎ 49%            ─  ${TITLE}`;
const STATUS_ROW = ROWS - 1;

// Shape of the v0.21.7 captures, reduced to the rows the reader can see.
const APPROVAL: Record<number, string> = {
	27: " ❯ Run the delete command",
	29: " └─ ▾ Tool calls (1)",
	30: " ╔══════════════════════════════════════════════════════════════════╗",
	31: " ║ ⚠ approval required · recursive delete                          │║",
	32: " ║  rm -rf ./example                                                 ║",
	33: " ║ ▸ 1. Allow once                                                   ║",
	34: " ║   2. Allow this session                                           ║",
	35: " ║   3. Always allow                                                 ║",
	36: " ║   4. Deny                                                         ║",
	37: " ║ ↑/↓ select · Enter confirm · 1-4 quick pick · Esc/Ctrl+C deny     ║",
	38: " ╚══════════════════════════════════════════════════════════════════╝",
	[STATUS_ROW]: BUSY("(¬_¬)", "contemplating"),
};

const read = (kind: HarnessKind, snap: ScreenSnapshot) => readScreenPhase(kind, snap);

describe("readScreenPhase / hermes", () => {
	it("an open approval box is waiting, though the status bar reads busy", () => {
		expect(read("hermes", screen(APPROVAL))).toBe("waiting");
	});

	it("an idle ready status line is waiting", () => {
		const snap = screen({ 36: " ❯", 38: " ┊  Done.", [STATUS_ROW]: READY });
		expect(read("hermes", snap)).toBe("waiting");
	});

	it("a draft typed in the composer is still waiting", () => {
		const snap = screen({ 36: " ❯ half a thought", [STATUS_ROW]: READY });
		expect(read("hermes", snap)).toBe("waiting");
	});

	it("a busy status line without a box is running, whatever the face and verb", () => {
		for (const [face, verb] of [
			["(◔_◔)", "deliberating"],
			["(¬_¬)", "contemplating"],
			["(•‿•)", "pondering"],
		] as const) {
			const snap = screen({ 36: " ❯ Ctrl+C to interrupt…", [STATUS_ROW]: BUSY(face, verb) });
			expect(read("hermes", snap)).toBe("running");
		}
	});

	it("an empty screen is unknown", () => {
		expect(read("hermes", screen({}))).toBe("unknown");
		expect(read("hermes", { rows: [], cursorX: 0, cursorY: 0 })).toBe("unknown");
	});

	it("garbled output is unknown", () => {
		const snap = screen({ 38: " {u uu", [STATUS_ROW]: " ─ ready" });
		expect(read("hermes", snap)).toBe("unknown");
	});

	it("a status line without the usage field is not recognised", () => {
		const snap = screen({ [STATUS_ROW]: " ─ ready │ haiku 4.5 │ nothing │ more" });
		expect(read("hermes", snap)).toBe("unknown");
	});

	it("approval phrases outside a box do not count", () => {
		const snap = screen({
			20: "approval required · recursive delete",
			21: "↑/↓ select · Esc/Ctrl+C deny",
		});
		expect(read("hermes", snap)).toBe("unknown");
	});

	it("a shell that prints `ready │` mid-screen is not waiting", () => {
		const snap = screen({
			30: "$ echo 'ready │ haiku 4.5 │ 1k/2k │ x'",
			31: "ready │ haiku 4.5 │ 1k/2k │ x",
			39: "$ ",
		});
		expect(read("hermes", snap)).toBe("unknown");
	});

	it("only the bottom rows are searched for the status line", () => {
		expect(read("hermes", screen({ 10: READY }))).toBe("unknown");
	});

	it("other kinds read unknown even on a hermes-looking screen", () => {
		for (const kind of ["claude", "opencode", "byoh"] as const) {
			expect(read(kind, screen(APPROVAL))).toBe("unknown");
			expect(read(kind, screen({ [STATUS_ROW]: READY }))).toBe("unknown");
		}
	});
});
