/// <reference types="node" />
// The gate, second version (#807): the daemon's redraw frames through
// @xterm/headless against xterm.js fed the raw recording, at the same cuts.
// Three comparisons per cut (see expectedFrames.ts for the pins):
//   A vs B-stream, B-stream vs B-full, B-stream vs B-skipped.

import { writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CATEGORIES, type CompareOptions, compare, type Diff, formatDiff } from "./compare.ts";
import { A_VS_STREAM, type Pins, STREAM_VS_FULL, STREAM_VS_SKIPPED } from "./expectedFrames.ts";
import { FIXTURES, loadFixture } from "./fixtures.ts";
import { loadFrames } from "./framesFixture.ts";
import { type ByCut, fullPictures, skippedPictures, streamPictures } from "./framesReplay.ts";
import { picturesAt } from "./replay.ts";

interface Comparison {
	label: string;
	pins: Pins;
	opts: CompareOptions;
	/** Which side the second picture comes from. */
	other: "stream" | "full" | "skipped";
}
// Both sides are xterm.js: nothing to excuse, and a link-underline excuse here
// would hide a real difference. History is skipped when the right side is on the
// alternate screen and so cannot hold the main screen (B-full only).
const COMPARISONS: Comparison[] = [
	{
		label: "A vs B-stream",
		pins: A_VS_STREAM,
		opts: { excuse: false, skipHistory: false },
		other: "stream",
	},
	{ label: "B-stream vs B-full", pins: STREAM_VS_FULL, opts: { excuse: false }, other: "full" },
	{
		label: "B-stream vs B-skipped",
		pins: STREAM_VS_SKIPPED,
		opts: { excuse: false, skipHistory: false },
		other: "skipped",
	},
];

const report: string[] = [];
let fullHistorySkipped = 0;
const tally = new Map<string, Map<string, string[]>>();

for (const name of FIXTURES) {
	describe(`frames ${name}`, () => {
		const frames = loadFrames(name);
		const { recording } = loadFixture(name);
		let a: Awaited<ReturnType<typeof picturesAt>>;
		const side = {} as Record<"stream" | "full" | "skipped", ByCut>;

		it("replays the recording and the three frame feeds", async () => {
			a = await picturesAt(
				recording,
				frames.cuts.map((c) => c.afterEvent),
			);
			side.stream = await streamPictures(frames);
			side.full = await fullPictures(frames);
			side.skipped = await skippedPictures(frames);
			expect(side.stream.size).toBe(frames.cuts.length);
		}, 120_000);

		for (const cmp of COMPARISONS) {
			for (const cut of frames.cuts) {
				it(`${cmp.label}: cut ${cut.name} differs exactly as pinned`, () => {
					const left = cmp.other === "stream" ? a.get(cut.afterEvent) : side.stream.get(cut.name);
					const right = side[cmp.other].get(cut.name);
					if (!left || !right) throw new Error("missing picture");
					const opts: CompareOptions = { ...cmp.opts };
					const { diffs, skippedHistory, excused } = compare(left, right, opts);
					if (cmp.other === "full" && skippedHistory) fullHistorySkipped++;
					expect(excused, "no link-underline excuse between two xterm.js terminals").toBe(0);
					const key = `${name}/${cut.name}`;
					const per = tally.get(cmp.label) ?? new Map<string, string[]>();
					tally.set(cmp.label, per);
					for (const d of diffs) per.set(d.category, [...(per.get(d.category) ?? []), key]);
					report.push(
						`${cmp.label} ${key}${skippedHistory ? " (history skipped)" : ""}${diffs.length ? `\n${diffs.map((d: Diff) => `  ${formatDiff(d)}`).join("\n")}` : ": identical"}`,
					);
					const got = diffs.map((d) => d.category).sort();
					const want = Object.keys(cmp.pins[key] ?? {}).sort();
					expect(got, diffs.map(formatDiff).join("\n")).toEqual(want);
				});
			}
		}
	});
}

describe("expected frames table", () => {
	it("names only real cuts and categories", () => {
		const cutKeys = new Set<string>();
		for (const n of FIXTURES) for (const c of loadFrames(n).cuts) cutKeys.add(`${n}/${c.name}`);
		for (const pins of [A_VS_STREAM, STREAM_VS_FULL, STREAM_VS_SKIPPED]) {
			for (const [key, cats] of Object.entries(pins)) {
				expect(cutKeys.has(key), key).toBe(true);
				for (const cat of Object.keys(cats)) expect(CATEGORIES as readonly string[]).toContain(cat);
			}
		}
	});

	it("covers every cut of the recordings' cut lists", () => {
		for (const n of FIXTURES) {
			expect(loadFrames(n).cuts.map((c) => [c.name, c.afterEvent])).toEqual(
				loadFixture(n).cuts.map((c) => [c.name, c.afterEvent]),
			);
		}
	});

	it("skips history only where B-full is on the alternate screen (12 cuts, as in T-1)", () => {
		expect(fullHistorySkipped).toBe(12);
	});

	it("summarises differing cuts per category, and writes a report when asked", () => {
		const lines: string[] = [];
		for (const cmp of COMPARISONS) {
			const per = tally.get(cmp.label) ?? new Map<string, string[]>();
			lines.push(cmp.label);
			for (const cat of CATEGORIES) {
				const cuts = per.get(cat) ?? [];
				if (cuts.length) lines.push(`  ${cat}: ${cuts.length} cuts: ${cuts.join(", ")}`);
			}
			if (per.size === 0) lines.push("  identical at every cut");
		}
		console.log(lines.join("\n"));
		const out = process.env.SKEIN_GATE_REPORT;
		if (out) writeFileSync(out, `${lines.join("\n")}\n\n${report.join("\n")}`);
	});
});
