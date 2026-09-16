import { describe, expect, it, vi } from "vitest";
import { type CoalescedResult, createCoalescer, sameJson } from "./coalesce.ts";

/// A controllable fake async fn: each call returns a new deferred promise,
/// and the test resolves/rejects them in whatever order it wants to.
function fakeAsync<T>() {
	const deferreds: { resolve: (v: T) => void; reject: (e: unknown) => void }[] = [];
	const calls: number[] = [];
	let nextCall = 0;
	const fn = () =>
		new Promise<T>((resolve, reject) => {
			calls.push(nextCall++);
			deferreds.push({ resolve, reject });
		});
	return {
		fn,
		callCount: () => calls.length,
		resolve: (index: number, value: T) => deferreds[index]?.resolve(value),
		reject: (index: number, error: unknown) => deferreds[index]?.reject(error),
	};
}

/// Flush the microtask queue enough times for chained `.then`s to settle.
const flush = async () => {
	for (let i = 0; i < 5; i++) await Promise.resolve();
};

describe("createCoalescer", () => {
	it("does not overlap: a second request while one is in flight does not start a new run", async () => {
		const fake = fakeAsync<number>();
		const results: CoalescedResult<number>[] = [];
		const coalescer = createCoalescer(fake.fn, (r) => results.push(r));

		coalescer.request();
		coalescer.request();
		expect(fake.callCount()).toBe(1);

		fake.resolve(0, 1);
		await flush();
		// The dirty flag from the second request() earns exactly one
		// trailing run, started only once the first settles.
		expect(fake.callCount()).toBe(2);
	});

	it("collapses many requests during one flight into a single trailing run", async () => {
		const fake = fakeAsync<number>();
		const results: CoalescedResult<number>[] = [];
		const coalescer = createCoalescer(fake.fn, (r) => results.push(r));

		coalescer.request();
		for (let i = 0; i < 20; i++) coalescer.request();
		expect(fake.callCount()).toBe(1);

		fake.resolve(0, 1);
		await flush();
		expect(fake.callCount()).toBe(2);

		fake.resolve(1, 2);
		await flush();
		// No further trailing run: nothing requested during the second flight.
		expect(fake.callCount()).toBe(2);
		expect(results).toEqual([
			{ ok: true, value: 1 },
			{ ok: true, value: 2 },
		]);
	});

	it("runs the trailing rerun even when the in-flight run errors", async () => {
		const fake = fakeAsync<number>();
		const results: CoalescedResult<number>[] = [];
		const coalescer = createCoalescer(fake.fn, (r) => results.push(r));

		coalescer.request();
		coalescer.request(); // marks dirty while the first is in flight
		fake.reject(0, new Error("boom"));
		await flush();

		expect(fake.callCount()).toBe(2);
		fake.resolve(1, 7);
		await flush();

		expect(results).toEqual([
			{ ok: false, error: new Error("boom") },
			{ ok: true, value: 7 },
		]);
	});

	it("discards a result that settles after dispose, as if the key had changed", async () => {
		const fake = fakeAsync<number>();
		const onSettle = vi.fn();
		const coalescer = createCoalescer(fake.fn, onSettle);

		coalescer.request();
		coalescer.dispose();
		fake.resolve(0, 1);
		await flush();

		expect(onSettle).not.toHaveBeenCalled();
	});

	it("dispose also cancels a pending trailing run", async () => {
		const fake = fakeAsync<number>();
		const onSettle = vi.fn();
		const coalescer = createCoalescer(fake.fn, onSettle);

		coalescer.request();
		coalescer.request(); // dirty
		coalescer.dispose();
		fake.resolve(0, 1);
		await flush();

		// The first run's own result is swallowed by dispose, and the
		// trailing run dispose was meant to cancel never starts.
		expect(fake.callCount()).toBe(1);
		expect(onSettle).not.toHaveBeenCalled();
	});

	it("request() after dispose is a no-op", async () => {
		const fake = fakeAsync<number>();
		const onSettle = vi.fn();
		const coalescer = createCoalescer(fake.fn, onSettle);
		coalescer.dispose();
		coalescer.request();
		await flush();
		expect(fake.callCount()).toBe(0);
	});
});

describe("sameJson", () => {
	it("treats equal plain objects as the same", () => {
		expect(sameJson({ a: 1, b: [1, 2] }, { a: 1, b: [1, 2] })).toBe(true);
	});

	it("treats differing objects as different", () => {
		expect(sameJson({ a: 1 }, { a: 2 })).toBe(false);
	});

	it("treats two undefineds as the same, without stringifying", () => {
		expect(sameJson(undefined, undefined)).toBe(true);
	});

	it("treats undefined and a value as different", () => {
		expect(sameJson(undefined, { a: 1 })).toBe(false);
	});
});
