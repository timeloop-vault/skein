import { beforeEach, describe, expect, it } from "vitest";
import { forget, isTerminalReply, noteInput, noteOutput, STALE_MS } from "./ptyLatency.ts";

describe("ptyLatency (#639)", () => {
	beforeEach(() => {
		forget("a");
		forget("b");
	});

	it("returns null when nothing is pending", () => {
		expect(noteOutput("a", 10)).toBeNull();
	});

	it("first keystroke wins", () => {
		noteInput("a", 100);
		noteInput("a", 150);
		expect(noteOutput("a", 180)).toBe(80);
	});

	it("clears after output", () => {
		noteInput("a", 100);
		expect(noteOutput("a", 120)).toBe(20);
		expect(noteOutput("a", 130)).toBeNull();
		noteInput("a", 200);
		expect(noteOutput("a", 215)).toBe(15);
	});

	it("keeps PTYs independent", () => {
		noteInput("a", 100);
		noteInput("b", 110);
		expect(noteOutput("b", 130)).toBe(20);
		expect(noteOutput("a", 140)).toBe(40);
	});

	it("stale output returns null and clears", () => {
		noteInput("a", 100);
		expect(noteOutput("a", 100 + STALE_MS + 1)).toBeNull();
		expect(noteOutput("a", 100 + STALE_MS + 2)).toBeNull();
	});

	it("stale pending is replaced by a new key", () => {
		noteInput("a", 100);
		noteInput("a", 100 + STALE_MS + 10);
		expect(noteOutput("a", 100 + STALE_MS + 30)).toBe(20);
	});

	it("isTerminalReply spots auto-replies only", () => {
		for (const s of [
			"\x1b[12;40R",
			"\x1b[?1;2c",
			"\x1b[>0;276;0c",
			"\x1b]11;rgb:0/0/0\x07",
			"\x1bP1$r0m\x1b\\",
		]) {
			expect(isTerminalReply(s)).toBe(true);
		}
		for (const s of ["a", "\x1b[A", "\x03", "\r", "\x1bOP"]) {
			expect(isTerminalReply(s)).toBe(false);
		}
	});

	it("forget drops pending state", () => {
		noteInput("a", 100);
		forget("a");
		expect(noteOutput("a", 120)).toBeNull();
	});
});
