import { describe, expect, it, vi } from "vitest";
import { guardChannelHandler } from "./harnessEvents.ts";

// #362: @tauri-apps/api's `Channel` calls `onmessage` BEFORE advancing
// `nextMessageIndex`, so a single throw from an unguarded handler parks
// every later message in `pendingMessages` forever — the channel wedges
// silently while Rust keeps sending. These tests exercise the extracted
// `guardChannelHandler` directly, rather than a real Tauri `Channel`,
// since that wedge lives in a dependency we can't fake cleanly.

describe("guardChannelHandler (#362)", () => {
	it("keeps handling later messages after an earlier one throws", () => {
		const seen: string[] = [];
		const handler = (event: { kind: string }) => {
			if (event.kind === "boom") throw new Error("kaboom");
			seen.push(event.kind);
		};
		const log = vi.fn();
		const guarded = guardChannelHandler("h1", "test_target", handler, log);

		guarded({ kind: "boom" });
		guarded({ kind: "after" });

		expect(seen).toEqual(["after"]);
	});

	it("logs the harness id, event kind and error message once per throw", () => {
		const handler = (): void => {
			throw new Error("kaboom");
		};
		const log = vi.fn();
		const guarded = guardChannelHandler("h1", "test_target", handler, log);

		guarded({ kind: "boom" });

		expect(log).toHaveBeenCalledTimes(1);
		const [level, target, message] = log.mock.calls[0] as [string, string, string];
		expect(level).toBe("error");
		expect(target).toBe("test_target");
		expect(message).toContain("harness=h1");
		expect(message).toContain("kind=boom");
		expect(message).toContain("kaboom");
	});

	it("rate-limits logging to the first 5 throws per attach", () => {
		const handler = (): void => {
			throw new Error("boom");
		};
		const log = vi.fn();
		const guarded = guardChannelHandler("h1", "test_target", handler, log);

		for (let i = 0; i < 10; i++) guarded({ kind: "x" });

		expect(log).toHaveBeenCalledTimes(5);
	});

	it("truncates a long stack to ~1500 chars", () => {
		const handler = (): void => {
			const err = new Error("boom");
			err.stack = "x".repeat(5000);
			throw err;
		};
		const log = vi.fn();
		const guarded = guardChannelHandler("h1", "test_target", handler, log);

		guarded({ kind: "x" });

		const message = log.mock.calls[0]?.[2] as string;
		const stackPart = message.split("stack: ")[1] ?? "";
		expect(stackPart.length).toBeLessThanOrEqual(1500);
	});
});
