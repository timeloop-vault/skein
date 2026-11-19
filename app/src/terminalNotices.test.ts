import { describe, expect, it } from "vitest";
import { createNoticeGate } from "./terminalNotices.ts";

describe("createNoticeGate", () => {
	it("writes local notices in raw mode", () => {
		const out: string[] = [];
		const gate = createNoticeGate((s) => out.push(s));
		gate.observe({ kind: "data" });
		gate.write("banner");
		expect(out).toEqual(["banner"]);
	});

	it("drops local notices once a grid event shows frames mode", () => {
		const out: string[] = [];
		const gate = createNoticeGate((s) => out.push(s));
		gate.observe({ kind: "grid" });
		gate.observe({ kind: "exit" });
		gate.write("banner");
		gate.write("sidecar line");
		expect(out).toEqual([]);
	});

	it("keeps writing what it was asked before frames mode was seen", () => {
		const out: string[] = [];
		const gate = createNoticeGate((s) => out.push(s));
		gate.write("early");
		gate.observe({ kind: "grid" });
		gate.write("late");
		expect(out).toEqual(["early"]);
	});
});
