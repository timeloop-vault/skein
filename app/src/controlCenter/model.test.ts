import { describe, expect, it } from "vitest";
import { buildStrip } from "../roomGroups.ts";
import type { Room } from "../types.ts";
import {
	buildControlCenter,
	type HarnessSnapshot,
	isTurnStart,
	parseStatusLine,
	type RoomSnapshot,
	roomAttention,
	roomLastActivity,
	type SignoffSnapshot,
} from "./model.ts";

function room(id: string, opts: Partial<Room> = {}): Room {
	return {
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [],
		activeHarnessId: "",
		...opts,
	};
}

function h(
	status: HarnessSnapshot["status"],
	opts: Partial<HarnessSnapshot> = {},
): HarnessSnapshot {
	return {
		id: `h-${status}`,
		name: "claude",
		kind: "claude",
		status,
		label: status,
		lastActivityAt: null,
		since: null,
		turnStartedAt: null,
		...opts,
	};
}

function snap(roomId: string, opts: Partial<RoomSnapshot> = {}): RoomSnapshot {
	return { roomId, harnesses: [], signoff: null, lastStatus: null, branch: null, ...opts };
}

function signoff(opts: Partial<SignoffSnapshot> = {}): SignoffSnapshot {
	return { approved: false, stale: false, unresolvedCount: 0, unaddressedCount: 0, ...opts };
}

function status(body: string, createdMs = 1000) {
	return { body, createdMs };
}

describe("parseStatusLine", () => {
	it("parses the em dash form", () => {
		expect(parseStatusLine("status: Review — ready to look")).toEqual({
			state: "review",
			task: "ready to look",
			line: "status: Review — ready to look",
		});
	});

	it("accepts a status line with no task part", () => {
		expect(parseStatusLine("status: review")).toEqual({
			state: "review",
			task: "",
			line: "status: review",
		});
	});

	it("accepts ' - ' and '--'", () => {
		expect(parseStatusLine("status: blocked - need key")?.task).toBe("need key");
		expect(parseStatusLine("status: blocked -- need key")?.state).toBe("blocked");
	});

	it("uses only the first line and trims it", () => {
		const p = parseStatusLine("  status: working — a\nmore text — here  ");
		expect(p?.line).toBe("status: working — a");
		expect(p?.task).toBe("a");
	});

	it("returns null for non status lines", () => {
		expect(parseStatusLine("hello")).toBeNull();
		expect(parseStatusLine("")).toBeNull();
		expect(parseStatusLine("done\nstatus: blocked — x")).toBeNull();
	});
});

describe("roomAttention", () => {
	it("permission is blocked", () => {
		const a = roomAttention(snap("r", { harnesses: [h("permission", { since: 5 })] }));
		expect(a).toEqual({
			rank: "blocked",
			reason: "permission",
			since: 5,
			harnessId: "h-permission",
		});
	});

	it("permission beats approved-stale review and waiting", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("waiting"), h("permission")],
				signoff: signoff({ approved: true, stale: true }),
			}),
		);
		expect(a.rank).toBe("blocked");
	});

	it("blocked status line blocks an idle room", () => {
		const a = roomAttention(
			snap("r", { harnesses: [h("idle")], lastStatus: status("status: blocked — need you", 42) }),
		);
		expect(a).toMatchObject({ rank: "blocked", reason: "blocked on you", since: 42 });
	});

	it("blocked status is ignored while a harness runs", () => {
		const a = roomAttention(
			snap("r", { harnesses: [h("running")], lastStatus: status("status: blocked — x") }),
		);
		expect(a.rank).toBe("working");
		const b = roomAttention(
			snap("r", { harnesses: [h("spawning")], lastStatus: status("status: blocked — x") }),
		);
		expect(b.rank).toBe("working");
	});

	it("stale approved sign-off is review and beats waiting", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("waiting")],
				signoff: signoff({ approved: true, stale: true }),
				lastStatus: status("status: done — x", 7),
			}),
		);
		expect(a).toMatchObject({ rank: "review", reason: "sign-off stale", since: 7 });
	});

	it("stale approved sign-off does not rank while a harness is running", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("running")],
				signoff: signoff({ approved: true, stale: true }),
			}),
		);
		expect(a.rank).toBe("working");
	});

	it("a review line is superseded by a later turn start", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("idle", { turnStartedAt: 20 })],
				lastStatus: status("status: review — x", 10),
			}),
		);
		expect(a.rank).toBe("idle");
		const b = roomAttention(
			snap("r", {
				harnesses: [h("waiting", { turnStartedAt: 20 })],
				lastStatus: status("status: blocked — x", 10),
			}),
		);
		expect(b.rank).toBe("waiting");
	});

	it("a review line survives its own turn ending (no new turn start)", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("waiting", { turnStartedAt: 5 })],
				lastStatus: status("status: review — x", 10),
			}),
		);
		expect(a.rank).toBe("review");
	});

	it("isTurnStart: permission -> running is not a new turn", () => {
		expect(isTurnStart("permission", "running")).toBe(false);
		expect(isTurnStart("waiting", "running")).toBe(true);
		expect(isTurnStart("idle", "running")).toBe(true);
		expect(isTurnStart("running", "waiting")).toBe(false);
	});

	it("review status line, with open thread count", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("idle")],
				signoff: signoff({ unresolvedCount: 3 }),
				lastStatus: status("status: review — pr", 9),
			}),
		);
		expect(a).toMatchObject({
			rank: "review",
			reason: "ready for review, 3 open threads",
			since: 9,
		});
		const b = roomAttention(snap("r", { lastStatus: status("status: review — pr", 9) }));
		expect(b.reason).toBe("ready for review");
	});

	it("review status is ignored while running", () => {
		const a = roomAttention(
			snap("r", { harnesses: [h("running")], lastStatus: status("status: review — x") }),
		);
		expect(a.rank).toBe("working");
	});

	it("review status is ignored when approved and current", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("idle")],
				signoff: signoff({ approved: true }),
				lastStatus: status("status: review — x"),
			}),
		);
		expect(a.rank).toBe("idle");
	});

	it("waiting uses the oldest waiting harness's since", () => {
		const a = roomAttention(
			snap("r", {
				harnesses: [h("waiting", { id: "a", since: 20 }), h("waiting", { id: "b", since: 10 })],
			}),
		);
		expect(a).toEqual({ rank: "waiting", reason: "your turn", since: 10, harnessId: "b" });
	});

	it("waiting beats working", () => {
		expect(roomAttention(snap("r", { harnesses: [h("running"), h("waiting")] })).rank).toBe(
			"waiting",
		);
	});

	it("working and idle report room last activity", () => {
		const w = roomAttention(snap("r", { harnesses: [h("running", { lastActivityAt: 50 })] }));
		expect(w).toMatchObject({ rank: "working", since: 50 });
		const i = roomAttention(snap("r", { harnesses: [h("exited"), h("idle", { since: 3 })] }));
		expect(i).toMatchObject({ rank: "idle", since: 3 });
	});

	it("harnessId names the harness driving the rank", () => {
		const shell = h("idle", { id: "shell" });
		expect(
			roomAttention(snap("r", { harnesses: [shell, h("waiting", { id: "cc2" })] })).harnessId,
		).toBe("cc2");
		expect(
			roomAttention(snap("r", { harnesses: [shell, h("permission", { id: "p" })] })).harnessId,
		).toBe("p");
		expect(
			roomAttention(snap("r", { harnesses: [shell, h("running", { id: "run" })] })).harnessId,
		).toBe("run");
		expect(
			roomAttention(snap("r", { harnesses: [shell, h("exited", { id: "x" })] })).harnessId,
		).toBe("shell");
		const rev = roomAttention(
			snap("r", { harnesses: [shell], lastStatus: status("status: review — x") }),
		);
		expect(rev).toMatchObject({ rank: "review", harnessId: "shell" });
	});

	it("no harnesses is idle", () => {
		expect(roomAttention(snap("r"))).toEqual({
			rank: "idle",
			reason: "idle",
			since: null,
			harnessId: null,
		});
	});
});

describe("roomLastActivity", () => {
	it("takes the max across harnesses and status", () => {
		const s = snap("r", {
			harnesses: [h("idle", { lastActivityAt: 5, since: 9 }), h("idle", { lastActivityAt: 7 })],
			lastStatus: status("x", 8),
		});
		expect(roomLastActivity(s)).toBe(9);
	});

	it("is null without data", () => {
		expect(roomLastActivity(snap("r"))).toBeNull();
	});
});

describe("buildControlCenter", () => {
	const waitingSnap = (id: string, since: number) =>
		snap(id, { harnesses: [h("waiting", { since })] });
	const workingSnap = (id: string, at: number) =>
		snap(id, { harnesses: [h("running", { lastActivityAt: at })] });
	const toMap = (...ss: RoomSnapshot[]) => new Map(ss.map((s) => [s.roomId, s]));

	it("sorts rows by rank across plain sections", () => {
		const segs = buildStrip([room("idle"), room("work"), room("wait"), room("perm")]);
		const out = buildControlCenter(
			segs,
			toMap(
				workingSnap("work", 5),
				waitingSnap("wait", 5),
				snap("perm", { harnesses: [h("permission", { since: 1 })] }),
			),
		);
		expect(out.map((s) => s.key)).toEqual(["perm", "wait", "work", "idle"]);
		expect(out.map((s) => s.rank)).toEqual(["blocked", "waiting", "working", "idle"]);
		expect(out.every((s) => !s.isGroup)).toBe(true);
	});

	it("waiting: oldest since first; working: most recent first", () => {
		const segs = buildStrip([room("a"), room("b"), room("c"), room("d")]);
		const out = buildControlCenter(
			segs,
			toMap(
				waitingSnap("a", 200),
				waitingSnap("b", 100),
				workingSnap("c", 10),
				workingSnap("d", 20),
			),
		);
		expect(out.map((s) => s.key)).toEqual(["b", "a", "d", "c"]);
	});

	it("falls back to strip order on ties", () => {
		const segs = buildStrip([room("a"), room("b"), room("c")]);
		const out = buildControlCenter(segs, toMap());
		expect(out.map((s) => s.key)).toEqual(["a", "b", "c"]);
	});

	it("keeps groups from buildStrip and ranks rows inside them", () => {
		const repo = "C:/Repo/Proj";
		const rooms = [
			room("main", { name: "Main", repoRoot: repo, cwd: repo }),
			room("wt1", { repoRoot: repo, cwd: "C:/Repo/Proj-wt/one" }),
			room("wt2", { repoRoot: repo, cwd: "C:/Repo/Proj-wt/two" }),
			room("solo"),
		];
		const segs = buildStrip(rooms);
		const out = buildControlCenter(
			segs,
			toMap(workingSnap("main", 1), waitingSnap("wt2", 5), workingSnap("solo", 99)),
		);
		expect(out.map((s) => s.key)).toHaveLength(2);
		const group = out[0];
		expect(group?.isGroup).toBe(true);
		expect(group?.label).toBe("Main");
		expect(group?.rank).toBe("waiting");
		expect(group?.rows.map((r) => r.room.id)).toEqual(["wt2", "main", "wt1"]);
		expect(out[1]?.key).toBe("solo");
	});

	it("uses the repo label when the group's lead is not open", () => {
		const repo = "C:/Repo/Proj";
		const rooms = [
			room("wt1", { repoRoot: repo, cwd: "C:/Repo/Proj-wt/one" }),
			room("wt2", { repoRoot: repo, cwd: "C:/Repo/Proj-wt/two" }),
		];
		const segs = buildStrip(rooms);
		const out = buildControlCenter(segs, toMap());
		expect(out).toHaveLength(1);
		expect(out[0]?.isGroup).toBe(true);
		expect(out[0]?.rows.map((r) => r.room.id)).toEqual(["wt1", "wt2"]);
	});

	it("lists rooms missing snapshots as idle", () => {
		const out = buildControlCenter(buildStrip([room("x")]), new Map());
		expect(out[0]?.rows[0]?.attention).toEqual({
			rank: "idle",
			reason: "idle",
			since: null,
			harnessId: null,
		});
		expect(out[0]?.rows[0]?.snap.roomId).toBe("x");
	});
});
