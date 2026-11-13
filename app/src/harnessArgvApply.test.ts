import { describe, expect, it } from "vitest";
import { applyResumedHarness, applyResumedRoom, applyResumedRooms } from "./harnessArgvApply.ts";
import type { Harness, Room } from "./types.ts";

const h = (over: Partial<Harness> = {}): Harness => ({
	id: "h1",
	kind: "claude",
	name: "H",
	status: "idle",
	model: "",
	tokens: "",
	...over,
});

const r = (harnesses: Harness[], over: Partial<Room> = {}): Room => ({
	id: "r1",
	name: "R",
	task: "",
	status: "idle",
	badge: 0,
	harnesses,
	activeHarnessId: harnesses[0]?.id ?? "",
	...over,
});

describe("applyResumedHarness", () => {
	it("takes cmd, launch and shellClaim only, never sessionId (#755)", () => {
		const mine = h({ cmd: ["old"], sessionId: "old", name: "mine", model: "m", spawnGen: 3 });
		const back = h({
			cmd: ["claude", "--resume", "s"],
			sessionId: "s",
			shellClaim: { sessionId: "s" },
			name: "daemon-name",
			model: "x",
			spawnGen: 9,
		});
		const out = applyResumedHarness(mine, back);
		expect(out).toMatchObject({
			cmd: ["claude", "--resume", "s"],
			sessionId: "old",
			shellClaim: { sessionId: "s" },
			name: "mine",
			model: "m",
			spawnGen: 3,
		});
	});

	it("removes a consumed shellClaim", () => {
		const mine = h({ cmd: ["zsh"], sessionId: "s", shellClaim: { sessionId: "s" } });
		const out = applyResumedHarness(mine, h({ cmd: ["claude", "--resume", "s"], sessionId: "s" }));
		expect(out).not.toHaveProperty("shellClaim");
		expect(out.cmd).toEqual(["claude", "--resume", "s"]);
	});

	it("copies launch back only when the daemon returned one", () => {
		const shell: Harness["launch"] = { mode: "shell", argv: ["bash"] };
		const kept = applyResumedHarness(h({ cmd: ["claude"] }), h({ cmd: ["claude"], launch: shell }));
		expect(kept.launch).toEqual(shell);
		const dropped = applyResumedHarness(
			h({ cmd: ["claude"], launch: shell }),
			h({ cmd: ["claude", "--resume", "s"] }),
		);
		expect(dropped).not.toHaveProperty("launch");
	});

	it("keeps cmd absent when the returned harness has none", () => {
		const out = applyResumedHarness(h({ kind: "files" }), h({ kind: "files" }));
		expect(out).not.toHaveProperty("cmd");
	});

	it("keeps the local sessionId whether or not the daemon returned one (#755)", () => {
		const out = applyResumedHarness(h({ sessionId: "stale", cmd: ["x"] }), h({ cmd: ["x"] }));
		expect(out.sessionId).toBe("stale");
	});

	it("keeps a field Rust does not know", () => {
		const mine = { ...h({ cmd: ["a"] }), futureField: { n: 1 } } as Harness;
		const out = applyResumedHarness(mine, h({ cmd: ["b"] })) as Harness & { futureField?: unknown };
		expect(out.futureField).toEqual({ n: 1 });
		expect(out.cmd).toEqual(["b"]);
	});

	it("returns the record untouched when nothing came back", () => {
		const mine = h({ cmd: ["a"] });
		expect(applyResumedHarness(mine, undefined)).toBe(mine);
	});
});

describe("applyResumedRoom", () => {
	it("merges harnesses by id and leaves an id missing from the result alone", () => {
		const a = h({ id: "a", cmd: ["a0"] });
		const b = h({ id: "b", cmd: ["b0"] });
		const room = r([a, b]);
		const out = applyResumedRoom(room, r([h({ id: "a", cmd: ["a1"] })]));
		expect(out.harnesses[0]?.cmd).toEqual(["a1"]);
		expect(out.harnesses[1]).toBe(b);
	});

	it("does not touch room fields without unarchive", () => {
		const room = r([h({ cmd: ["a"] })], {
			archived: 5,
			retired: 6,
			nameSet: true,
		} as Partial<Room>);
		const out = applyResumedRoom(room, r([h({ cmd: ["b"] })]));
		expect(out.archived).toBe(5);
		expect(out.retired).toBe(6);
	});

	it("unarchive drops archived, retired and closedBy and keeps the rest", () => {
		const room = {
			...r([h({ cmd: ["a"] })], {
				archived: 5,
				retired: 6,
				closedBy: { roomId: "x", harnessId: "y", at: 5 },
				name: "keep",
			}),
			futureRoomField: 1,
		} as Room;
		const out = applyResumedRoom(room, r([h({ cmd: ["b"] })]), { unarchive: true });
		expect(out).not.toHaveProperty("archived");
		expect(out).not.toHaveProperty("retired");
		expect(out).not.toHaveProperty("closedBy");
		expect(out.name).toBe("keep");
		expect(out).toHaveProperty("futureRoomField", 1);
		expect(out.harnesses[0]?.cmd).toEqual(["b"]);
	});

	it("a room missing from the result is untouched", () => {
		const room = r([h({ cmd: ["a"] })], { archived: 5 });
		expect(applyResumedRoom(room, undefined, { unarchive: true })).toBe(room);
	});
});

describe("applyResumedRooms", () => {
	it("matches rooms by id", () => {
		const one = r([h({ cmd: ["a"] })], { id: "one" });
		const two = r([h({ cmd: ["z"] })], { id: "two" });
		const out = applyResumedRooms([one, two], [r([h({ cmd: ["b"] })], { id: "one" })]);
		expect(out[0]?.harnesses[0]?.cmd).toEqual(["b"]);
		expect(out[1]).toBe(two);
	});
});
