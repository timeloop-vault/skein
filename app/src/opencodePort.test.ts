import { describe, expect, it } from "vitest";
import { opencodePortFromCmd, opencodePortsFromRooms } from "./opencodePort.ts";
import type { Room } from "./types.ts";

describe("opencodePortFromCmd", () => {
	it("reads the separate and the = form", () => {
		expect(opencodePortFromCmd(["opencode", "--port", "4097"])).toBe(4097);
		expect(opencodePortFromCmd(["opencode", "--port=4098", "--x"])).toBe(4098);
	});

	it("returns null when absent, empty or without a value", () => {
		expect(opencodePortFromCmd([])).toBeNull();
		expect(opencodePortFromCmd(["opencode"])).toBeNull();
		expect(opencodePortFromCmd(["opencode", "--port"])).toBeNull();
		expect(opencodePortFromCmd(["opencode", "--port="])).toBeNull();
	});

	it("rejects non-numeric values", () => {
		expect(opencodePortFromCmd(["--port", "abc"])).toBeNull();
		expect(opencodePortFromCmd(["--port", "12a"])).toBeNull();
		expect(opencodePortFromCmd(["--port", "-5"])).toBeNull();
		expect(opencodePortFromCmd(["--port=4.5"])).toBeNull();
	});

	it("rejects out-of-range values: 0 and above 65535; accepts the bounds", () => {
		expect(opencodePortFromCmd(["--port", "0"])).toBeNull();
		expect(opencodePortFromCmd(["--port", "65536"])).toBeNull();
		expect(opencodePortFromCmd(["--port", "1"])).toBe(1);
		expect(opencodePortFromCmd(["--port", "65535"])).toBe(65535);
	});

	it("skips an invalid occurrence and keeps looking", () => {
		expect(opencodePortFromCmd(["--port", "x", "--port=4000"])).toBe(4000);
	});
});

const h = (id: string, kind: string, cmd?: string[]) => ({ id, kind, cmd });
const room = (id: string, harnesses: unknown[]) => ({ id, harnesses }) as unknown as Room;

describe("opencodePortsFromRooms", () => {
	it("maps opencode harnesses with a port only", () => {
		const ports = opencodePortsFromRooms([
			room("r1", [
				h("a", "opencode", ["opencode", "--port", "4001"]),
				h("b", "opencode", ["opencode"]),
				h("c", "opencode"),
				h("d", "claude", ["claude", "--port", "4002"]),
			]),
			room("r2", [h("e", "opencode", ["opencode", "--port=4003"])]),
		]);
		expect([...ports.entries()]).toEqual([
			["a", 4001],
			["e", 4003],
		]);
	});

	it("is empty for no rooms", () => {
		expect(opencodePortsFromRooms([]).size).toBe(0);
	});
});
