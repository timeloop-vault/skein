import { beforeEach, describe, expect, it } from "vitest";
import { claudeVersionStore as s } from "./claudeVersionStore.ts";

beforeEach(() => s.reset());

describe("claudeVersionStore", () => {
	it("ignores a running version when no spawn is known", () => {
		s.setInstalled("2.1.288");
		s.recordRunning("h", "2.1.280", 5000);
		expect(s.notice("h")).toBeNull();
	});

	it("ignores rows before spawn and null timestamps", () => {
		s.setInstalled("2.1.288");
		s.markSpawned("h", 1000);
		s.recordRunning("h", "2.1.200", 999);
		s.recordRunning("h", "2.1.200", null);
		expect(s.notice("h")).toBeNull();
		s.recordRunning("h", "2.1.280", 1000);
		expect(s.notice("h")).toEqual({ running: "2.1.280", installed: "2.1.288" });
	});

	it("respawn clears running and refusal but keeps autoRestartedFor", () => {
		s.setInstalled("2.1.288");
		s.markSpawned("h", 1000);
		s.recordRunning("h", "2.1.280", 2000);
		s.setRefusal("h", "busy");
		s.markAutoRestarted("h", "2.1.288");
		s.markSpawned("h", 3000);
		expect(s.notice("h")).toBeNull();
		expect(s.refusal("h")).toBeNull();
		expect(s.autoRestartedFor("h")).toBe("2.1.288");
		s.recordRunning("h", "2.1.280", 2500);
		expect(s.notice("h")).toBeNull();
	});

	it("notifies subscribers and forget drops state", () => {
		let n = 0;
		const off = s.subscribe(() => n++);
		s.markSpawned("h", 1);
		s.setInstalled("1.0.0");
		expect(n).toBe(2);
		s.forget("h");
		off();
		s.setInstalled("1.0.1");
		expect(n).toBe(3);
		expect(s.autoRestartedFor("h")).toBeNull();
	});
});
