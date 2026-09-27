import { describe, expect, it } from "vitest";
import {
	type NewToast,
	type ToastEntry,
	appendToast,
	coalesceToast,
	toastRemainingMs,
} from "./toastStack.ts";

function newToast(id: string, opts: Partial<NewToast> = {}): NewToast {
	return {
		id,
		roomId: "r1",
		harnessId: "h1",
		kind: "claude",
		roomName: "room",
		harnessName: "harness",
		state: "waiting",
		...opts,
	};
}

describe("appendToast", () => {
	it("stamps expiresAt from now + TOAST_DISMISS_MS", () => {
		const now = 1_000;
		const next = appendToast([], newToast("a"), now);
		expect(next).toHaveLength(1);
		expect(next[0]?.expiresAt).toBe(now + 6_000);
	});

	it("caps the stack at TOAST_MAX_VISIBLE, dropping the oldest", () => {
		let stack: ToastEntry[] = [];
		for (let i = 0; i < 7; i++) {
			stack = appendToast(stack, newToast(`t${i}`), 1_000);
		}
		expect(stack).toHaveLength(5);
		expect(stack.map((t) => t.id)).toEqual(["t2", "t3", "t4", "t5", "t6"]);
	});

	it("does not touch an existing entry's expiresAt when a later toast is appended", () => {
		const first = appendToast([], newToast("a"), 1_000);
		const second = appendToast(first, newToast("b"), 5_000);
		expect(second[0]?.id).toBe("a");
		expect(second[0]?.expiresAt).toBe(first[0]?.expiresAt);
		expect(second[0]?.expiresAt).toBe(1_000 + 6_000);
	});
});

describe("coalesceToast", () => {
	it("keeps id and expiresAt while updating the patched fields", () => {
		const stack = appendToast([], newToast("a", { state: "error", detail: "first" }), 1_000);
		const next = coalesceToast(stack, 0, { detail: "second" });
		expect(next[0]?.id).toBe("a");
		expect(next[0]?.expiresAt).toBe(stack[0]?.expiresAt);
		expect(next[0]?.detail).toBe("second");
	});

	it("returns the same array when the index is out of range", () => {
		const stack = appendToast([], newToast("a"), 1_000);
		expect(coalesceToast(stack, 5, { detail: "x" })).toBe(stack);
	});
});

describe("toastRemainingMs", () => {
	it("returns the difference when expiry is in the future", () => {
		expect(toastRemainingMs(5_000, 1_000)).toBe(4_000);
	});

	it("clamps at 0 when expiry has already passed", () => {
		expect(toastRemainingMs(1_000, 5_000)).toBe(0);
	});

	it("clamps at 0 when expiry equals now", () => {
		expect(toastRemainingMs(1_000, 1_000)).toBe(0);
	});
});
