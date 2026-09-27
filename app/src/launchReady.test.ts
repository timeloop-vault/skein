import { describe, expect, it } from "vitest";
import { LAUNCH_QUIET_MS, LAUNCH_READY_CAP_MS, launchSettled } from "./launchReady.ts";
import type { LaunchSettledInput } from "./launchReady.ts";

const input = (over: Partial<LaunchSettledInput> = {}): LaunchSettledInput => ({
	adapterHeard: false,
	launchSignalAt: 0,
	lastOutputAt: null,
	nowMs: 0,
	...over,
});

describe("launchSettled", () => {
	it("settles immediately once the adapter has heard from the transcript", () => {
		expect(
			launchSettled(input({ adapterHeard: true, launchSignalAt: 0, lastOutputAt: 0, nowMs: 0 })),
		).toEqual({ settled: true });
	});

	it("settles immediately when there was never a launch ping", () => {
		expect(launchSettled(input({ launchSignalAt: null, nowMs: 0 }))).toEqual({ settled: true });
	});

	it("is not settled right after output, well inside the quiet window", () => {
		// Quiet is measured from `lastOutputAt` (200), not the ping (0), so
		// no time has elapsed against the quiet window yet at nowMs=200.
		const result = launchSettled(input({ launchSignalAt: 0, lastOutputAt: 200, nowMs: 200 }));
		expect(result).toEqual({ settled: false, retryInMs: LAUNCH_QUIET_MS });
	});

	it("counts down retryInMs as quiet accumulates after the last output", () => {
		const result = launchSettled(input({ launchSignalAt: 0, lastOutputAt: 200, nowMs: 400 }));
		expect(result).toEqual({ settled: false, retryInMs: LAUNCH_QUIET_MS - 200 });
	});

	it("settles once quiet has held for at least LAUNCH_QUIET_MS", () => {
		const result = launchSettled(
			input({ launchSignalAt: 0, lastOutputAt: 200, nowMs: 200 + LAUNCH_QUIET_MS }),
		);
		expect(result).toEqual({ settled: true });
	});

	it("settles at the cap even if output kept arriving right up to it", () => {
		const result = launchSettled(
			input({
				launchSignalAt: 0,
				lastOutputAt: LAUNCH_READY_CAP_MS - 1,
				nowMs: LAUNCH_READY_CAP_MS,
			}),
		);
		expect(result).toEqual({ settled: true });
	});

	it("measures quiet from the launch ping when lastOutputAt predates it", () => {
		// A stale `lastOutputAt` left over from before this launch must not
		// pull `quietSince` backwards — quiet is measured from the ping.
		const result = launchSettled(
			input({ launchSignalAt: 1000, lastOutputAt: 10, nowMs: 1000 + LAUNCH_QUIET_MS }),
		);
		expect(result).toEqual({ settled: true });

		const notYet = launchSettled(
			input({ launchSignalAt: 1000, lastOutputAt: 10, nowMs: 1000 + LAUNCH_QUIET_MS - 1 }),
		);
		expect(notYet).toEqual({ settled: false, retryInMs: 1 });
	});
});
