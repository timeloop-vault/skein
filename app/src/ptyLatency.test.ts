import { beforeEach, describe, expect, it, vi } from "vitest";

const recordSpan = vi.hoisted(() => vi.fn());
vi.mock("./perfTelemetry.ts", () => ({ recordSpan }));

import {
	classifyInput,
	echoPhase,
	forget,
	isTerminalReply,
	noteInput,
	noteOutput,
	recordEcho,
	STALE_MS,
} from "./ptyLatency.ts";

describe("ptyLatency (#639)", () => {
	beforeEach(() => {
		forget("a");
		forget("b");
		recordSpan.mockClear();
	});

	it("returns null when nothing is pending", () => {
		expect(noteOutput("a", 10)).toBeNull();
	});

	it("first keystroke wins, with its kind and phase", () => {
		noteInput("a", 100, "key", "waiting");
		noteInput("a", 150, "enter", "running");
		expect(noteOutput("a", 180)).toEqual({ ms: 80, kind: "key", phase: "waiting" });
	});

	it("clears after output", () => {
		noteInput("a", 100, "key", "other");
		expect(noteOutput("a", 120)?.ms).toBe(20);
		expect(noteOutput("a", 130)).toBeNull();
		noteInput("a", 200, "key", "other");
		expect(noteOutput("a", 215)?.ms).toBe(15);
	});

	it("keeps PTYs independent", () => {
		noteInput("a", 100, "key", "other");
		noteInput("b", 110, "key", "other");
		expect(noteOutput("b", 130)?.ms).toBe(20);
		expect(noteOutput("a", 140)?.ms).toBe(40);
	});

	it("stale output returns null and clears", () => {
		noteInput("a", 100, "key", "other");
		expect(noteOutput("a", 100 + STALE_MS + 1)).toBeNull();
		expect(noteOutput("a", 100 + STALE_MS + 2)).toBeNull();
	});

	it("stale pending is replaced by a new key", () => {
		noteInput("a", 100, "key", "other");
		noteInput("a", 100 + STALE_MS + 10, "ctrl", "running");
		expect(noteOutput("a", 100 + STALE_MS + 30)).toEqual({
			ms: 20,
			kind: "ctrl",
			phase: "running",
		});
	});

	it("recordEcho records the aggregate, kind and phase spans", () => {
		noteInput("a", 100, "mouse", "running");
		recordEcho("a", 125);
		expect(recordSpan.mock.calls).toEqual([
			["pty.echo", 25, "wall"],
			["pty.echo.mouse", 25, "wall"],
			["pty.echo@running", 25, "wall"],
		]);
		recordEcho("a", 130);
		recordEcho(null, 130);
		expect(recordSpan).toHaveBeenCalledTimes(3);
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
		noteInput("a", 100, "key", "other");
		forget("a");
		expect(noteOutput("a", 120)).toBeNull();
	});
});

describe("classifyInput", () => {
	const table: [string, string][] = [
		["a", "key"],
		["abc", "key"],
		["å", "key"],
		["\r", "enter"],
		["\n", "enter"],
		["\r\n", "enter"],
		["\x7f", "key"],
		["\b", "key"],
		["\t", "key"],
		["\x1bb", "key"],
		["abcdefghij", "paste"],
		["a\rb", "ctrl"],
		["line one\rline two\rxy", "paste"],
		["\x03", "ctrl"],
		["\x1b", "ctrl"],
		["\x1b[A", "ctrl"],
		["\x1b[<0;10;5M", "mouse"],
		["\x1b[<0;10;5m", "mouse"],
		["\x1b[<35;10;5M\x1b[<35;11;5M", "mouse"],
		["\x1b[M !!", "mouse"],
	];
	it.each(table)("%j -> %s", (data, kind) => {
		expect(classifyInput(data)).toBe(kind);
	});

	it("echoPhase buckets", () => {
		expect(echoPhase("waiting")).toBe("waiting");
		expect(echoPhase("running")).toBe("running");
		for (const p of ["permission", "idle", "spawning", undefined, null]) {
			expect(echoPhase(p)).toBe("other");
		}
	});
});
