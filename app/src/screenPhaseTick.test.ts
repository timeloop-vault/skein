import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { applyUpdate, resetPhaseMirror } from "./phaseMirror.ts";
import { phaseUpdate, wireActivity } from "./phaseTestSupport.ts";
import {
	resetScreenReadings,
	setScreenPhaseReader,
	tickScreenReadings,
} from "./screenPhaseTick.ts";

const sent = (): unknown[] =>
	vi
		.mocked(invoke)
		.mock.calls.filter(([cmd]) => cmd === "harness_phase_input")
		.map(([, args]) => (args as { input: unknown }).input);

beforeEach(() => {
	resetPhaseMirror();
	resetScreenReadings();
	vi.mocked(invoke).mockClear();
	setScreenPhaseReader(() => "waiting");
});

describe("tickScreenReadings", () => {
	it("forwards a reading once while it is unchanged", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { authoritative: false }) }));
		tickScreenReadings();
		tickScreenReadings();
		expect(sent()).toEqual([{ kind: "screenReading", reading: "waiting" }]);
	});

	it("re-sends an unchanged reading after a respawn (new spawnedAt, same id)", () => {
		applyUpdate(
			phaseUpdate("a", {
				view: wireActivity("running", { authoritative: false, spawnedAt: 1 }),
			}),
		);
		tickScreenReadings();
		applyUpdate(
			phaseUpdate("a", {
				view: wireActivity("running", { authoritative: false, spawnedAt: 2 }),
			}),
		);
		tickScreenReadings();
		tickScreenReadings();
		expect(sent()).toHaveLength(2);
	});

	it("tells the daemon to drop the reading of a harness it forgot", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { authoritative: false }) }));
		tickScreenReadings();
		applyUpdate(phaseUpdate("a", { view: null }));
		tickScreenReadings();
		expect(sent()).toEqual([
			{ kind: "screenReading", reading: "waiting" },
			{ kind: "screenReading", reading: null },
		]);
	});
});
