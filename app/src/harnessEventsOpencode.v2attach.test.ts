import { beforeEach, describe, expect, it, vi } from "vitest";

// #677 — `sidecarGeneration` rides the invoke only for v2; v1 args are unchanged.

const h = vi.hoisted(() => ({
	invoke: vi.fn((..._a: unknown[]) => new Promise<void>(() => {})),
}));

vi.mock("@tauri-apps/api/core", () => ({
	Channel: class {
		onmessage: (e: unknown) => void = () => {};
	},
	invoke: h.invoke,
}));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { harnessActivity } from "./harnessActivity.ts";
import { attachOpencodeEvents } from "./harnessEvents.ts";

const attachWith = (id: string, generation?: number): Record<string, unknown> => {
	harnessActivity.spawned(id);
	attachOpencodeEvents(
		id,
		"r",
		"/cwd",
		0,
		undefined,
		undefined,
		() => undefined,
		undefined,
		generation,
	);
	return h.invoke.mock.calls[0]?.[1] as Record<string, unknown>;
};

describe("opencode_events_attach args", () => {
	beforeEach(() => {
		h.invoke.mockClear();
	});

	it("passes sidecarGeneration for v2", () => {
		expect(attachWith("v2a", 5)).toMatchObject({ sidecarGeneration: 5 });
		expect(h.invoke.mock.calls[0]?.[0]).toBe("opencode_events_attach");
	});

	it("omits the key for v1", () => {
		expect("sidecarGeneration" in attachWith("v1a")).toBe(false);
	});
});
