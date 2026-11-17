import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("../frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { mkActivity } from "../phaseTestSupport.ts";
import { meaningfulActivityAt } from "./useControlCenterData.ts";

describe("meaningfulActivityAt", () => {
	it("is null without a record", () => {
		expect(meaningfulActivityAt(null)).toBeNull();
	});

	it("uses turn signals and phase changes for an agent with a turn signal", () => {
		const a = mkActivity("waiting", {
			lastTurnSignal: { kind: "end", at: 500 },
			phaseSince: 400,
			lastSubmitAt: 100,
			spawnedAt: 900,
		});
		expect(meaningfulActivityAt(a)).toBe(500);
	});

	it("uses only submit, spawn and exit for a harness with no turn signal", () => {
		const running = mkActivity("running", { phaseSince: 800, spawnedAt: 10, lastSubmitAt: 50 });
		expect(meaningfulActivityAt(running)).toBe(50);
		const exited = mkActivity("exited", { phaseSince: 800, spawnedAt: 10 });
		expect(meaningfulActivityAt(exited)).toBe(800);
	});
});
