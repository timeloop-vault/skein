import { afterEach, describe, expect, it, vi } from "vitest";
import {
	diffRooms,
	type RoomChange,
	type RoomMutation,
	type RoomsApplyOutcome,
	type RoomsChanged,
} from "./roomMutations.ts";
import { applyMutations } from "./roomStoreModel.testutil.ts";
import { createRoomsSync, type RoomsSync } from "./roomsSync.ts";
import type { Harness, Room } from "./types.ts";

const harness = (id: string): Harness => ({ id, kind: "shell", name: id }) as unknown as Harness;
const room = (id: string, extra: Record<string, unknown> = {}): Room =>
	({
		id,
		name: id,
		task: "",
		status: "idle",
		badge: 0,
		harnesses: [harness("h1")],
		activeHarnessId: "h1",
		...extra,
	}) as unknown as Room;
const put = (r: Room): RoomChange => ({ type: "roomPut", room: r });
const renamed = (id: string, name: string) => room(id, { name });
const tick = () => new Promise<void>((r) => setTimeout(r, 0));
const settle = async () => {
	for (let i = 0; i < 5; i++) await tick();
};

interface Call {
	mutations: RoomMutation[];
	resolve: (o?: RoomsApplyOutcome) => void;
	reject: (e: unknown) => void;
}

function harnessFor(opts: { canSync?: () => boolean } = {}) {
	const calls: Call[] = [];
	const timers: { fn: () => void; ms: number; id: number; cleared: boolean }[] = [];
	const errors: string[] = [];
	const log: string[] = [];
	let inFlight = 0;
	let maxInFlight = 0;
	const flushDaemon = vi.fn(async () => {
		log.push("flushDaemon");
	});
	const sync = createRoomsSync({
		clientId: "me",
		apply: (mutations) =>
			new Promise<RoomsApplyOutcome>((res, rej) => {
				inFlight++;
				maxInFlight = Math.max(maxInFlight, inFlight);
				log.push("apply");
				calls.push({
					mutations,
					resolve: (o = { skipped: [] }) => {
						inFlight--;
						res(o);
					},
					reject: (e) => {
						inFlight--;
						rej(e);
					},
				});
			}),
		flushDaemon,
		...(opts.canSync ? { canSync: opts.canSync } : {}),
		onError: (what) => errors.push(what),
		retryMs: 1000,
		setTimer: (fn, ms) => {
			const t = { fn, ms, id: timers.length, cleared: false };
			timers.push(t);
			return t;
		},
		clearTimer: (h) => {
			(h as { cleared: boolean }).cleared = true;
		},
	});
	return {
		sync,
		calls,
		timers,
		errors,
		log,
		flushDaemon,
		maxInFlight: () => maxInFlight,
		live: () => timers.filter((t) => !t.cleared),
	};
}

afterEach(() => {
	vi.restoreAllMocks();
});

describe("createRoomsSync: sending", () => {
	it("sends nothing before setBaseline or while canSync() is false (#167 parked autosave)", async () => {
		let ok = false;
		const h = harnessFor({ canSync: () => ok });
		h.sync.update([room("a")]);
		await settle();
		expect(h.calls).toHaveLength(0);
		h.sync.setBaseline([]);
		h.sync.update([room("a")]);
		await settle();
		expect(h.calls).toHaveLength(0);
		ok = true;
		h.sync.update([room("a")]);
		await settle();
		expect(h.calls).toHaveLength(1);
	});

	it("first update after setBaseline sends the diff", async () => {
		const h = harnessFor();
		const base = [room("a")];
		const next = [room("a"), room("b")];
		h.sync.setBaseline(base);
		h.sync.update(next);
		expect(h.calls).toHaveLength(1);
		expect(h.calls[0]?.mutations).toEqual(diffRooms(base, next));
	});

	it("an empty diff sends nothing", async () => {
		const h = harnessFor();
		h.sync.setBaseline([room("a")]);
		h.sync.update([room("a")]);
		await settle();
		expect(h.calls).toHaveLength(0);
	});

	it("changes made during a call collapse into one following call, never overlapping", async () => {
		const h = harnessFor();
		const a = room("a");
		h.sync.setBaseline([a]);
		const r1 = [a, room("b")];
		const r3 = [a, room("b"), room("c")];
		h.sync.update(r1);
		h.sync.update([a, room("b"), room("x")]);
		h.sync.update(r3);
		expect(h.calls).toHaveLength(1);
		h.calls[0]?.resolve();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations).toEqual(diffRooms(r1, r3));
		h.calls[1]?.resolve();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.maxInFlight()).toBe(1);
	});

	it("moves the baseline only on success", async () => {
		const h = harnessFor();
		const base = [room("a")];
		const next = [room("a"), room("b")];
		h.sync.setBaseline(base);
		h.sync.update(next);
		h.calls[0]?.resolve();
		await settle();
		// Baseline is now `next`: re-reporting it sends nothing.
		h.sync.update([room("a"), room("b")]);
		await settle();
		expect(h.calls).toHaveLength(1);
	});
});

describe("createRoomsSync: failure and retry", () => {
	it("leaves the baseline, calls onError and retries after the delay", async () => {
		const h = harnessFor();
		const base = [room("a")];
		const next = [room("a"), room("b")];
		h.sync.setBaseline(base);
		h.sync.update(next);
		h.calls[0]?.reject(new Error("boom"));
		await settle();
		expect(h.errors).toEqual(["rooms_apply"]);
		expect(h.live()).toHaveLength(1);
		expect(h.live()[0]?.ms).toBe(1000);
		h.live()[0]?.fn();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations).toEqual(h.calls[0]?.mutations);
		expect(h.calls[1]?.mutations).toEqual(diffRooms(base, next));
	});

	it("the retry sends the then-latest diff", async () => {
		const h = harnessFor();
		const base = [room("a")];
		const r2 = [room("a"), room("b"), room("c")];
		h.sync.setBaseline(base);
		h.sync.update([room("a"), room("b")]);
		h.sync.update(r2); // arrives while the first call is in flight
		h.calls[0]?.reject(new Error("boom"));
		await settle();
		expect(h.live()).toHaveLength(1);
		h.live()[0]?.fn();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations).toEqual(diffRooms(base, r2));
	});

	it("dispose stops a pending retry", async () => {
		const h = harnessFor();
		h.sync.setBaseline([room("a")]);
		h.sync.update([room("a"), room("b")]);
		h.calls[0]?.reject(new Error("boom"));
		await settle();
		const t = h.live()[0];
		expect(t).toBeDefined();
		h.sync.dispose();
		expect(h.live()).toHaveLength(0);
		t?.fn();
		await settle();
		expect(h.calls).toHaveLength(1);
	});
});

describe("createRoomsSync: remote", () => {
	const ev = (origin: string, changes: RoomChange[]): RoomsChanged => ({ origin, changes });

	it("returns null for the own clientId, empty changes and before a baseline", () => {
		const h = harnessFor();
		expect(h.sync.remote(ev("other", [put(room("a"))]))).toBeNull();
		h.sync.setBaseline([room("a")]);
		expect(h.sync.remote(ev("me", [put(renamed("a", "x"))]))).toBeNull();
		expect(h.sync.remote(ev("other", []))).toBeNull();
		expect(h.sync.remote(ev("other", [put(renamed("a", "x"))]))).not.toBeNull();
	});

	it("a foreign event moves the baseline so the change is not echoed back", async () => {
		const h = harnessFor();
		const a = room("a");
		h.sync.setBaseline([a]);
		const up = h.sync.remote(ev("other", [put(renamed("a", "A2"))]));
		const local = up?.([a]) ?? [];
		expect(local[0]?.name).toBe("A2");
		h.sync.update(local);
		await settle();
		expect(h.calls).toHaveLength(0);
	});

	it("re-applies a foreign event received in flight on top of the sent snapshot", async () => {
		const h = harnessFor();
		const a = room("a");
		h.sync.setBaseline([a]);
		const mine = [a, room("b")];
		h.sync.update(mine);
		expect(h.calls).toHaveLength(1);
		const up = h.sync.remote(ev("other", [put(renamed("a", "A2"))]));
		const local = up?.(mine) ?? [];
		expect(local.map((r) => r.name)).toEqual(["A2", "b"]);
		h.sync.update(local);
		h.calls[0]?.resolve();
		await settle();
		// No echo of A2 and no revert of it: the create of b was the only call.
		expect(h.calls).toHaveLength(1);
		// The baseline holds both: re-reporting a local with a different name sends only that.
		h.sync.update([renamed("a", "A3"), room("b")]);
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations.map((m) => m.type)).toEqual(["updateRoom"]);
	});

	it("the returned function is pure and does not move the baseline again", async () => {
		const h = harnessFor();
		const a = room("a");
		h.sync.setBaseline([a]);
		const up = h.sync.remote(ev("other", [put(renamed("a", "A2"))]));
		const r1 = up?.([a]);
		const r2 = up?.([a]);
		expect(r1).toEqual(r2);
		expect(r1?.[0]?.name).toBe("A2");
		h.sync.update(r1 ?? []);
		await settle();
		expect(h.calls).toHaveLength(0);
	});
});

describe("createRoomsSync: flush", () => {
	it("drains the chain, sends the latest diff, then flushes the daemon once", async () => {
		const h = harnessFor();
		const a = room("a");
		h.sync.setBaseline([a]);
		h.sync.update([a, room("b")]);
		h.sync.update([a, room("b"), room("c")]);
		const done = h.sync.flush();
		await settle();
		expect(h.flushDaemon).not.toHaveBeenCalled();
		h.calls[0]?.resolve();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.flushDaemon).not.toHaveBeenCalled();
		h.calls[1]?.resolve();
		await done;
		expect(h.flushDaemon).toHaveBeenCalledTimes(1);
		expect(h.log).toEqual(["apply", "apply", "flushDaemon"]);
	});

	it("flushes the daemon even when there is nothing to send", async () => {
		const h = harnessFor();
		h.sync.setBaseline([room("a")]);
		await h.sync.flush();
		expect(h.calls).toHaveLength(0);
		expect(h.flushDaemon).toHaveBeenCalledTimes(1);
	});

	it("never rejects when the daemon flush fails", async () => {
		const h = harnessFor();
		h.flushDaemon.mockRejectedValueOnce(new Error("x"));
		await expect(h.sync.flush()).resolves.toBeUndefined();
		expect(h.errors).toEqual(["rooms_flush"]);
	});
});

describe("createRoomsSync: flush with a pending retry", () => {
	it("sends the latest diff at once, does not double-send, then flushes the daemon", async () => {
		const h = harnessFor();
		const base = [room("a")];
		h.sync.setBaseline(base);
		h.sync.update([room("a"), room("b")]);
		h.calls[0]?.reject(new Error("boom"));
		await settle();
		const timer = h.live()[0];
		expect(timer).toBeDefined();

		const done = h.sync.flush();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations).toEqual(diffRooms(base, [room("a"), room("b")]));
		expect(h.live()).toHaveLength(0);
		expect(h.flushDaemon).not.toHaveBeenCalled();
		// The old timer firing now must not send a second copy.
		timer?.fn();
		await settle();
		expect(h.calls).toHaveLength(2);
		h.calls[1]?.resolve();
		await done;
		expect(h.calls).toHaveLength(2);
		expect(h.flushDaemon).toHaveBeenCalledTimes(1);
		expect(h.log[h.log.length - 1]).toBe("flushDaemon");
		expect(h.maxInFlight()).toBe(1);
	});

	it("sends the newest snapshot when updates arrived before the flush", async () => {
		const h = harnessFor();
		const base = [room("a")];
		h.sync.setBaseline(base);
		h.sync.update([room("a"), room("b")]);
		h.sync.update([room("a"), room("b"), room("c")]); // swallowed by the in-flight call
		h.calls[0]?.reject(new Error("boom"));
		await settle();
		const done = h.sync.flush();
		await settle();
		expect(h.calls).toHaveLength(2);
		expect(h.calls[1]?.mutations).toEqual(diffRooms(base, [room("a"), room("b"), room("c")]));
		h.calls[1]?.resolve();
		await done;
		expect(h.flushDaemon).toHaveBeenCalledTimes(1);
	});
});

describe("createRoomsSync: remote clears and failing calls", () => {
	it("a foreign roomPut that clears a field is not echoed back by the next update", async () => {
		const h = harnessFor();
		const a = room("a", { archived: 5 });
		h.sync.setBaseline([a]);
		const up = h.sync.remote({ origin: "other", changes: [put(room("a"))] });
		const local = up?.([a]) ?? [];
		expect((local[0] as unknown as Record<string, unknown>).archived).toBeUndefined();
		h.sync.update(local);
		await settle();
		expect(h.calls).toHaveLength(0);
	});

	it("repeated update() while calls keep failing never overlaps calls", async () => {
		const h = harnessFor();
		h.sync.setBaseline([room("a")]);
		for (let i = 0; i < 4; i++) {
			const rooms = [room("a"), room(`n${i}`)];
			h.sync.update(rooms);
			h.sync.update(rooms); // in flight: swallowed
			await settle();
			expect(h.maxInFlight()).toBe(1);
			h.calls[h.calls.length - 1]?.reject(new Error("boom"));
			await settle();
			expect(h.live()).toHaveLength(1);
		}
		expect(h.calls).toHaveLength(4);
		expect(h.maxInFlight()).toBe(1);
	});
});

describe("createRoomsSync: outcome", () => {
	it("warns about skipped mutations", async () => {
		const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
		const h = harnessFor();
		h.sync.setBaseline([room("a")]);
		h.sync.update([room("a"), room("b")]);
		h.calls[0]?.resolve({ skipped: [{ index: 0, reason: "exists" }] });
		await settle();
		expect(warn).toHaveBeenCalledTimes(1);
		expect(String(warn.mock.calls[0]?.[0])).toContain("exists");
	});
});

describe("two clients over one store", () => {
	it("converge on the model and neither erases the other's change", async () => {
		let model: Room[] = [room("a"), room("b")];
		const syncs: Record<string, RoomsSync> = {};
		const locals: Record<string, Room[]> = {};
		const make = (id: string, other: string) => {
			const s = createRoomsSync({
				clientId: id,
				flushDaemon: async () => {},
				apply: async (mutations) => {
					await tick();
					const out = applyMutations(model, mutations);
					model = out.rooms;
					if (out.changes.length > 0) {
						const up = syncs[other]?.remote({ origin: id, changes: out.changes });
						if (up) {
							locals[other] = up(locals[other] ?? []);
							syncs[other]?.update(locals[other]);
						}
					}
					return { skipped: [] };
				},
			});
			syncs[id] = s;
			locals[id] = [room("a"), room("b")];
			s.setBaseline(locals[id]);
		};
		make("A", "B");
		make("B", "A");

		locals.A = [...(locals.A ?? []), room("c")];
		syncs.A?.update(locals.A);
		locals.B = (locals.B ?? []).map((r) => (r.id === "b" ? renamed("b", "B2") : r));
		syncs.B?.update(locals.B);
		await settle();
		await syncs.A?.flush();
		await syncs.B?.flush();

		expect(model.map((r) => r.id)).toEqual(["a", "b", "c"]);
		expect(model.find((r) => r.id === "b")?.name).toBe("B2");
		expect(locals.A).toEqual(model);
		expect(locals.B).toEqual(model);
	});
});

describe("createRoomsSync: hydrate buffering", () => {
	const ev = (origin: string, changes: RoomChange[]): RoomsChanged => ({ origin, changes });
	const r1 = () => room("r1");
	const r1x = () => room("r1", { name: "r1-transformed" });
	const ids = (rs: readonly Room[]) => rs.map((r) => r.id);

	it("regression: a room created by another client during the load is not removed by the first update", async () => {
		const h = harnessFor();
		h.sync.beginHydrate();
		expect(h.sync.remote(ev("other", [put(room("r2"))]))).toBeNull();
		const installed = h.sync.finishHydrate([r1()], [r1x()]);
		expect(ids(installed)).toEqual(["r1", "r2"]);
		expect(installed[0]?.name).toBe("r1-transformed");
		h.sync.update(installed);
		const sent = h.calls[0]?.mutations ?? [];
		expect(sent).toEqual(diffRooms([r1(), room("r2")], installed));
		expect(sent.some((m) => m.type === "removeRoom")).toBe(false);
		expect(sent.some((m) => m.type === "createRoom")).toBe(false);
		expect(sent.some((m) => m.type === "updateRoom")).toBe(true);
	});

	it("merges a foreign change to the same room with its local transform", () => {
		const h = harnessFor();
		h.sync.beginHydrate();
		h.sync.remote(ev("other", [put(room("r1", { task: "T" }))]));
		const installed = h.sync.finishHydrate([r1()], [r1x()]);
		expect(installed[0]?.name).toBe("r1-transformed");
		expect(installed[0]?.task).toBe("T");
		h.sync.update(installed);
		const expected = diffRooms([room("r1", { task: "T" })], installed);
		expect(h.calls[0]?.mutations).toEqual(expected);
		expect(expected.length).toBeGreaterThan(0);
	});

	it("sends nothing between beginHydrate and finishHydrate", async () => {
		const h = harnessFor();
		h.sync.setBaseline([r1()]);
		h.sync.beginHydrate();
		h.sync.update([r1x()]);
		await settle();
		expect(h.calls).toHaveLength(0);
	});

	it("with nothing queued returns the same local reference and baselines on rows", () => {
		const h = harnessFor();
		const rows = [r1()];
		const local = [r1x()];
		h.sync.beginHydrate();
		expect(h.sync.finishHydrate(rows, local)).toBe(local);
		h.sync.update(local);
		expect(h.calls[0]?.mutations).toEqual(diffRooms(rows, local));
	});

	it("an event the loaded rows already reflect changes nothing", async () => {
		const h = harnessFor();
		const rows = [r1(), room("r2")];
		h.sync.beginHydrate();
		h.sync.remote(ev("other", [put(room("r2"))]));
		const local = [...rows];
		const out = h.sync.finishHydrate(rows, local);
		expect(out).toEqual(local);
		h.sync.update(out);
		await settle();
		expect(h.calls).toHaveLength(0);
	});

	it("ignores own-origin echoes while buffering", () => {
		const h = harnessFor();
		h.sync.beginHydrate();
		expect(h.sync.remote(ev("me", [put(room("r2"))]))).toBeNull();
		const local = [r1()];
		expect(h.sync.finishHydrate([r1()], local)).toBe(local);
	});

	it("a second beginHydrate (retry) discards queued events", () => {
		const h = harnessFor();
		h.sync.beginHydrate();
		h.sync.remote(ev("other", [put(room("r2"))]));
		h.sync.beginHydrate();
		const local = [r1x()];
		const out = h.sync.finishHydrate([r1()], local);
		expect(out).toBe(local);
		h.sync.update(out);
		expect(h.calls[0]?.mutations).toEqual(diffRooms([r1()], local));
	});

	it("after finishHydrate, remote works normally again", () => {
		const h = harnessFor();
		h.sync.beginHydrate();
		h.sync.finishHydrate([r1()], [r1()]);
		const up = h.sync.remote(ev("other", [put(room("r3"))]));
		expect(typeof up).toBe("function");
		expect(ids(up?.([r1()]) ?? [])).toEqual(["r1", "r3"]);
	});
});
