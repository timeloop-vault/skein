import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// #404: this module is the only one allowed to import `invoke` directly
// for logging purposes, so it's the only place that needs a
// `@tauri-apps/api/core` mock — every caller goes through `logToRust`/
// `logBoth` instead.
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
	invoke: (...args: unknown[]) => invokeMock(...args),
}));

const { logBoth, logToRust } = await import("./frontendLog.ts");

describe("logToRust (#362, #404)", () => {
	beforeEach(() => {
		invokeMock.mockReset();
	});

	it("forwards level, target and message to the frontend_log command", () => {
		invokeMock.mockReturnValue(Promise.resolve());
		logToRust("info", "skein::seam", "hello");
		expect(invokeMock).toHaveBeenCalledWith("frontend_log", {
			level: "info",
			target: "skein::seam",
			message: "hello",
		});
	});

	it("swallows a rejection from invoke", async () => {
		invokeMock.mockReturnValue(Promise.reject(new Error("no runtime")));
		expect(() => logToRust("warn", "skein::mail", "boom")).not.toThrow();
		// Let the rejected promise's `.catch` microtask run.
		await Promise.resolve();
		await Promise.resolve();
	});

	it("swallows a synchronous throw from invoke", () => {
		invokeMock.mockImplementation(() => {
			throw new Error("no window");
		});
		expect(() => logToRust("error", "skein::activity", "boom")).not.toThrow();
	});
});

describe("logBoth (#404)", () => {
	beforeEach(() => {
		invokeMock.mockReset();
		invokeMock.mockReturnValue(Promise.resolve());
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	it("writes an info line to console.info and forwards it", () => {
		const spy = vi.spyOn(console, "info").mockImplementation(() => {});
		logBoth("info", "skein::seam", "[skein] hello");
		expect(spy).toHaveBeenCalledWith("[skein] hello");
		expect(invokeMock).toHaveBeenCalledWith("frontend_log", {
			level: "info",
			target: "skein::seam",
			message: "[skein] hello",
		});
	});

	it("writes a warn line to console.warn and forwards it", () => {
		const spy = vi.spyOn(console, "warn").mockImplementation(() => {});
		logBoth("warn", "skein::mail", "[skein] uh oh");
		expect(spy).toHaveBeenCalledWith("[skein] uh oh");
		expect(invokeMock).toHaveBeenCalledWith("frontend_log", {
			level: "warn",
			target: "skein::mail",
			message: "[skein] uh oh",
		});
	});

	it("writes an error line to console.error and forwards it", () => {
		const spy = vi.spyOn(console, "error").mockImplementation(() => {});
		logBoth("error", "skein::activity", "[skein] broken");
		expect(spy).toHaveBeenCalledWith("[skein] broken");
		expect(invokeMock).toHaveBeenCalledWith("frontend_log", {
			level: "error",
			target: "skein::activity",
			message: "[skein] broken",
		});
	});

	it("does not throw when invoke rejects", () => {
		vi.spyOn(console, "info").mockImplementation(() => {});
		invokeMock.mockReturnValue(Promise.reject(new Error("no runtime")));
		expect(() => logBoth("info", "skein::seam", "[skein] hello")).not.toThrow();
	});

	it("does not throw when invoke throws synchronously", () => {
		vi.spyOn(console, "info").mockImplementation(() => {});
		invokeMock.mockImplementation(() => {
			throw new Error("no window");
		});
		expect(() => logBoth("info", "skein::seam", "[skein] hello")).not.toThrow();
	});
});
