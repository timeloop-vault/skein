import { describe, expect, it } from "vitest";
import { bufferPtyInput } from "./ptyInputBuffer.ts";

function fakeTerm() {
	const subs = new Set<(d: string) => void>();
	return {
		term: {
			onData(cb: (d: string) => void) {
				subs.add(cb);
				return { dispose: () => void subs.delete(cb) };
			},
		},
		emit: (d: string) => {
			for (const cb of [...subs]) cb(d);
		},
		count: () => subs.size,
	};
}

describe("bufferPtyInput", () => {
	it("collects in order before take", () => {
		const f = fakeTerm();
		const b = bufferPtyInput(f.term);
		f.emit("\x1b[1;1R");
		f.emit("a");
		expect(b.takeAndDispose()).toEqual(["\x1b[1;1R", "a"]);
	});

	it("take disposes: later data is not collected", () => {
		const f = fakeTerm();
		const b = bufferPtyInput(f.term);
		f.emit("a");
		expect(b.takeAndDispose()).toEqual(["a"]);
		expect(f.count()).toBe(0);
		f.emit("b");
		expect(b.takeAndDispose()).toEqual([]);
	});

	it("dispose drops what was collected", () => {
		const f = fakeTerm();
		const b = bufferPtyInput(f.term);
		f.emit("a");
		b.dispose();
		expect(f.count()).toBe(0);
		expect(b.takeAndDispose()).toEqual([]);
	});

	it("take on empty returns empty", () => {
		const f = fakeTerm();
		expect(bufferPtyInput(f.term).takeAndDispose()).toEqual([]);
	});
});
