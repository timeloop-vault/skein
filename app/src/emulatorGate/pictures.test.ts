/// <reference types="node" />
// The gate (#793, slice T-1): xterm.js after replaying a recording against the
// Rust engine's committed pictures at every cut. A cut passes when the set of
// differing categories equals the expected set (expected.ts): an unexpected
// difference fails, and so does a pin that no longer differs.

import { writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { CATEGORIES, compare, type Diff, formatDiff } from "./compare.ts";
import { loadEnginePictures } from "./enginePicture.ts";
import { EXPECTED } from "./expected.ts";
import { FIXTURES, loadFixture } from "./fixtures.ts";
import { picturesAt } from "./replay.ts";

const results: Array<{ key: string; diffs: Diff[]; skipped: boolean; excused: number }> = [];

/** Cells the link-underline rule excused over all 24 cuts (pinned: it must not widen or go dead). */
const EXCUSED_TOTAL = 186;

for (const name of FIXTURES) {
	describe(`pictures ${name}`, () => {
		const { recording, cuts } = loadFixture(name);
		const engine = loadEnginePictures(name);
		let xterm: Awaited<ReturnType<typeof picturesAt>>;

		it("replays through xterm.js", async () => {
			xterm = await picturesAt(
				recording,
				cuts.map((c) => c.afterEvent),
			);
			expect(xterm.size).toBe(cuts.length);
			expect(engine.size).toBe(cuts.length);
		}, 120_000);

		for (const cut of cuts) {
			it(`cut ${cut.name} differs exactly as pinned`, () => {
				const a = xterm.get(cut.afterEvent);
				const b = engine.get(cut.afterEvent);
				if (!a || !b) throw new Error("missing picture");
				const { diffs, skippedHistory, excused } = compare(a, b);
				const hasLinks = [...a.screen, ...a.history, ...b.screen, ...b.history].some((r) =>
					r.runs.some((u) => u.link),
				);
				if (!hasLinks) expect(excused, "no links, nothing excused").toBe(0);
				const key = `${name}/${cut.name}`;
				results.push({ key, diffs, skipped: skippedHistory, excused });
				const got = diffs.map((d) => d.category).sort();
				const want = Object.keys(EXPECTED[key] ?? {}).sort();
				expect(got, diffs.map(formatDiff).join("\n")).toEqual(want);
			});
		}
	});
}

describe("expected table", () => {
	it("names only real cuts and categories", () => {
		const cutKeys = new Set<string>();
		for (const n of FIXTURES) for (const c of loadFixture(n).cuts) cutKeys.add(`${n}/${c.name}`);
		for (const [key, cats] of Object.entries(EXPECTED)) {
			expect(cutKeys.has(key), key).toBe(true);
			for (const cat of Object.keys(cats)) expect(CATEGORIES as readonly string[]).toContain(cat);
		}
	});

	it("has the pinned number of UNEXPLAINED entries", () => {
		const n = Object.values(EXPECTED)
			.flatMap((c) => Object.values(c))
			.filter((m) => m?.startsWith("UNEXPLAINED:")).length;
		expect(n).toBe(0);
	});

	it("counts the cuts whose history comparison is skipped for the alternate screen", () => {
		// The engine has no history while the alternate screen is active; the
		// claude-session never leaves it from event 16, the scrollback-session
		// has the one in-alt-screen cut.
		const skipped = FIXTURES.flatMap((n) =>
			[...loadEnginePictures(n)].filter(([, p]) => p.altScreen).map(([ev]) => `${n}@${ev}`),
		);
		expect(skipped.length).toBe(12);
		expect(results.filter((r) => r.skipped).length).toBe(12);
	});

	it("pins the cells the link-underline rule excused", () => {
		const total = results.reduce((n, r) => n + r.excused, 0);
		expect(total).toBeGreaterThan(0);
		expect(total).toBe(EXCUSED_TOTAL);
	});

	it("writes a report when asked", () => {
		const out = process.env.SKEIN_GATE_REPORT;
		if (!out) return;
		const lines = results.map((r) =>
			r.diffs.length === 0
				? `${r.key}: identical${r.skipped ? " (history skipped)" : ""} [excused ${r.excused}]`
				: `${r.key}${r.skipped ? " (history skipped)" : ""} [excused ${r.excused}]:\n${r.diffs.map((d) => `  ${formatDiff(d)}`).join("\n")}`,
		);
		writeFileSync(out, lines.join("\n"));
	});
});
