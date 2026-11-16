import { describe, expect, it } from "vitest";
import { spawnOutcome } from "./ptySpawnOutcome.ts";

const none = { sidecarGeneration: null };

describe("spawnOutcome (#730, #754)", () => {
	it("a sidecar generation attaches by generation, with no port", () => {
		const o = spawnOutcome({ sidecarGeneration: 7, opencodePort: 4100 });
		expect(o.sidecarGeneration).toBe(7);
		expect(o.port).toBeUndefined();
	});

	it("no generation attaches by the ensure result's opencodePort", () => {
		const o = spawnOutcome({ ...none, opencodePort: 4100 });
		expect(o.sidecarGeneration).toBeUndefined();
		expect(o.port).toBe(4100);
	});

	it("a null or absent port yields no port", () => {
		expect(spawnOutcome({ ...none, opencodePort: null }).port).toBeUndefined();
		expect(spawnOutcome(none).port).toBeUndefined();
	});
});
