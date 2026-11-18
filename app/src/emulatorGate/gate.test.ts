/// <reference types="node" />
// Replay groundwork for the emulator gate (#793): every fixture loads, its
// cuts lie inside the recording and replay to the last event without error,
// and the baseline terminal scrolls back as far as the Rust engine keeps.

import { describe, expect, it } from "vitest";
import { FIXTURES, loadFixture, rustHistoryRows } from "./fixtures.ts";
import { baselinesAt } from "./replay.ts";
import { makeTerm } from "./terminals.ts";

for (const name of FIXTURES) {
	describe(`replay ${name}`, () => {
		const { recording, cuts } = loadFixture(name);

		it("replays to its last event without error", async () => {
			const last = recording.events.length - 1;
			const out = await baselinesAt(recording, [last]);
			expect(out.get(last)?.cols).toBeGreaterThan(0);
		}, 120_000);

		it("has every cut inside the recording", () => {
			expect(cuts.length).toBeGreaterThan(0);
			for (const c of cuts) {
				expect(c.afterEvent, c.name).toBeGreaterThanOrEqual(0);
				expect(c.afterEvent, c.name).toBeLessThan(recording.events.length);
			}
		});
	});
}

it("the baseline's scrollback is the Rust HISTORY_ROWS", () => {
	expect(makeTerm(80, 24).term.options.scrollback).toBe(rustHistoryRows());
});
