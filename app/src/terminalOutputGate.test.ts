import { describe, expect, it } from "vitest";
import { createOutputGate, type OutputGateOptions } from "./terminalOutputGate.ts";

function setup(hidden: boolean, opts: OutputGateOptions = {}) {
	const written: string[] = [];
	const parsed: Array<() => void> = [];
	const sink = {
		write(data: string, onParsed?: () => void) {
			written.push(data);
			if (onParsed) parsed.push(onParsed);
		},
	};
	let now = 0;
	let nextId = 1;
	const pending = new Map<number, { at: number; fn: () => void }>();
	const timers = {
		setTimeout(fn: () => void, ms: number) {
			const id = nextId++;
			pending.set(id, { at: now + ms, fn });
			return id;
		},
		clearTimeout(h: unknown) {
			pending.delete(h as number);
		},
	};
	const advance = (ms: number) => {
		now += ms;
		for (const [id, t] of [...pending]) {
			if (t.at <= now) {
				pending.delete(id);
				t.fn();
			}
		}
	};
	const gate = createOutputGate(sink, hidden, { timers, ...opts });
	return { gate, written, parsed, advance, pending };
}

describe("createOutputGate", () => {
	it("passes visible writes straight through in order", () => {
		const { gate, written } = setup(false);
		gate.write("a");
		gate.write("b");
		expect(written).toEqual(["a", "b"]);
		expect(gate.bufferedChars).toBe(0);
	});

	it("buffers while hidden and leaves the sink untouched", () => {
		const { gate, written } = setup(true);
		gate.write("abc");
		gate.write("de");
		expect(written).toEqual([]);
		expect(gate.bufferedChars).toBe(5);
	});

	it("flushes after the stream has been quiet", () => {
		const { gate, written, advance } = setup(true, { idleFlushMs: 300 });
		gate.write("a");
		advance(299);
		expect(written).toEqual([]);
		advance(1);
		expect(written).toEqual(["a"]);
		expect(gate.bufferedChars).toBe(0);
	});

	it("re-arms the idle timer on new data", () => {
		const { gate, written, advance } = setup(true, { idleFlushMs: 300 });
		gate.write("a");
		advance(200);
		gate.write("b");
		advance(200);
		expect(written).toEqual([]);
		advance(100);
		expect(written).toEqual(["ab"]);
	});

	it("flushes on overflow without dropping anything", () => {
		const { gate, written, pending } = setup(true, { maxBufferedChars: 5 });
		gate.write("abc");
		gate.write("def");
		expect(written.join("")).toBe("abcdef");
		expect(gate.bufferedChars).toBe(0);
		expect(pending.size).toBe(0);
	});

	it("flushes on reveal", () => {
		const { gate, written } = setup(true);
		gate.write("a");
		gate.setHidden(false);
		expect(written).toEqual(["a"]);
		gate.write("b");
		expect(written).toEqual(["a", "b"]);
	});

	it("preserves order across hide and reveal", () => {
		const { gate, written, advance } = setup(false);
		gate.write("1");
		gate.setHidden(true);
		gate.write("2");
		gate.write("3");
		gate.setHidden(false);
		gate.write("4");
		gate.setHidden(true);
		gate.write("5");
		advance(1000);
		expect(written.join("")).toBe("12345");
	});

	it("cuts a flush into slices", () => {
		const { gate, written } = setup(true, { sliceChars: 4 });
		gate.write("abcdef");
		gate.write("ghij");
		gate.flush();
		expect(written).toEqual(["abcd", "efgh", "ij"]);
	});

	it("never splits a surrogate pair", () => {
		const { gate, written } = setup(true, { sliceChars: 4 });
		gate.write("abc\u{1F600}de");
		gate.flush();
		expect(written).toEqual(["abc", "\u{1F600}de"]);
		for (const w of written) {
			const last = w.charCodeAt(w.length - 1);
			expect(last >= 0xd800 && last <= 0xdbff).toBe(false);
		}
	});

	describe("isSettled", () => {
		it("is false while buffered or flushed slices are unparsed, true after", () => {
			const { gate, parsed } = setup(true);
			expect(gate.isSettled()).toBe(true);
			gate.write("a");
			expect(gate.isSettled()).toBe(false);
			gate.flush();
			expect(gate.isSettled()).toBe(false);
			parsed[0]?.();
			expect(gate.isSettled()).toBe(true);
		});
	});

	describe("whenDrained", () => {
		it("is synchronous when nothing is buffered or in flight", () => {
			const { gate } = setup(false);
			let called = 0;
			gate.whenDrained(() => called++);
			expect(called).toBe(1);
		});

		it("is synchronous while visible pass-through writes are in flight", () => {
			const { gate } = setup(false);
			gate.write("a");
			let called = 0;
			gate.whenDrained(() => called++);
			expect(called).toBe(1);
			expect(gate.isSettled()).toBe(true);
		});

		it("waits for onParsed of flushed slices", () => {
			const { gate, parsed } = setup(true, { sliceChars: 2 });
			gate.write("abcd");
			gate.flush();
			let called = 0;
			gate.whenDrained(() => called++);
			expect(called).toBe(0);
			parsed[0]?.();
			expect(called).toBe(0);
			parsed[1]?.();
			expect(called).toBe(1);
		});

		it("flushes buffered data first", () => {
			const { gate, written, parsed } = setup(true);
			gate.write("a");
			let called = 0;
			gate.whenDrained(() => called++);
			expect(written).toEqual(["a"]);
			expect(called).toBe(0);
			parsed[0]?.();
			expect(called).toBe(1);
		});

		it("a throwing waiter does not drop the rest", () => {
			const { gate, parsed } = setup(true);
			gate.write("a");
			gate.flush();
			let called = 0;
			gate.whenDrained(() => {
				throw new Error("boom");
			});
			gate.whenDrained(() => called++);
			parsed[0]?.();
			expect(called).toBe(1);
		});

		it("fires several waiters in order", () => {
			const { gate, parsed } = setup(true);
			gate.write("a");
			gate.flush();
			const order: number[] = [];
			gate.whenDrained(() => order.push(1));
			gate.whenDrained(() => order.push(2));
			parsed[0]?.();
			expect(order).toEqual([1, 2]);
		});
	});

	describe("dispose", () => {
		it("makes writes, flush and setHidden no-ops", () => {
			const { gate, written, advance } = setup(true);
			gate.write("a");
			gate.dispose();
			expect(gate.bufferedChars).toBe(0);
			advance(1000);
			gate.write("b");
			gate.flush();
			gate.setHidden(false);
			gate.write("c");
			expect(written).toEqual([]);
		});

		it("drops pending waiters and ignores late onParsed", () => {
			const { gate, parsed } = setup(true);
			gate.write("a");
			gate.flush();
			let called = 0;
			gate.whenDrained(() => called++);
			gate.dispose();
			parsed[0]?.();
			expect(called).toBe(0);
		});

		it("clears the idle timer", () => {
			const { gate, pending } = setup(true);
			gate.write("a");
			expect(pending.size).toBe(1);
			gate.dispose();
			expect(pending.size).toBe(0);
		});
	});
});
