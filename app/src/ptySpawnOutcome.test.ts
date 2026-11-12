import { describe, expect, it } from "vitest";
import { spawnOutcome } from "./ptySpawnOutcome.ts";

const none = { sidecarGeneration: null, opencodeVersion: null, sessionId: null };
const v2 = { major: 2, version: "2.0.0" };

describe("spawnOutcome (#730)", () => {
	it("a sidecar generation attaches by generation, with no port", () => {
		const o = spawnOutcome({ ...none, sidecarGeneration: 7, opencodeVersion: v2 }, "opencode", {
			sessionId: "s",
			opencodePort: 4100,
		});
		expect(o.sidecarGeneration).toBe(7);
		expect(o.port).toBeUndefined();
		expect(o.opencodeVersion).toEqual(v2);
	});

	it("no generation attaches by the record's opencodePort", () => {
		const o = spawnOutcome(none, "opencode", { sessionId: "s", opencodePort: 4100 });
		expect(o.sidecarGeneration).toBeUndefined();
		expect(o.port).toBe(4100);
		expect(o.capturedSessionId).toBeUndefined();
	});

	it("a session id differing from the record is captured; the same one is not", () => {
		const r = { sidecarGeneration: 1, opencodeVersion: v2, sessionId: "ses_new" };
		expect(spawnOutcome(r, "opencode", { sessionId: undefined, opencodePort: 1 })).toMatchObject({
			capturedSessionId: "ses_new",
		});
		expect(spawnOutcome(r, "opencode", { sessionId: "ses_old", opencodePort: 1 })).toMatchObject({
			capturedSessionId: "ses_new",
		});
		expect(
			spawnOutcome(r, "opencode", { sessionId: "ses_new", opencodePort: 1 }).capturedSessionId,
		).toBeUndefined();
	});

	it("an opencode spawn whose probe found nothing reports null, to delete the stored version", () => {
		const o = spawnOutcome(none, "opencode", { sessionId: undefined, opencodePort: undefined });
		expect(o.opencodeVersion).toBeNull();
	});

	it("other kinds never report an opencode version", () => {
		const o = spawnOutcome({ ...none, opencodeVersion: v2 }, "claude", {
			sessionId: undefined,
			opencodePort: undefined,
		});
		expect(o.opencodeVersion).toBeUndefined();
	});
});
