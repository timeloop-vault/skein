import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./frontendLog.ts", () => ({ logBoth: vi.fn(), logToRust: vi.fn() }));

import { harnessActivity } from "./harnessActivity.ts";
import {
	applySnapshot,
	applyUpdate,
	FAILURE_LOG_INTERVAL_MS,
	type MirrorDeps,
	onFirstSnapshot,
	type PhaseUpdate,
	phaseMirror,
	resetPhaseMirror,
	resetPhaseMirrorStart,
	SNAPSHOT_ATTEMPTS,
	SNAPSHOT_RETRY_MS,
	SNAPSHOT_SLOW_RETRY_MS,
	startPhaseMirror,
} from "./phaseMirror.ts";
import {
	phaseUpdate,
	subagentEntry,
	taskEntry,
	transitionOf,
	wireActivity,
} from "./phaseTestSupport.ts";

beforeEach(() => {
	resetPhaseMirror();
	resetPhaseMirrorStart();
});

describe("applyUpdate", () => {
	it("replaces the record, and deletes it on view: null", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("running") }));
		expect(harnessActivity.get("a")?.phase).toBe("running");
		applyUpdate(phaseUpdate("a", { view: wireActivity("waiting") }));
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
		applyUpdate(phaseUpdate("a", { view: null }));
		expect(harnessActivity.get("a")).toBeNull();
		expect(harnessActivity.phaseSnapshot()).toEqual({});
	});

	it("drops an update whose seq is not above the last applied for that harness", () => {
		const first = phaseUpdate("a", { view: wireActivity("running") });
		const second = phaseUpdate("a", { view: wireActivity("waiting") });
		expect(applyUpdate(second)).toBe(true);
		expect(applyUpdate(first)).toBe(false);
		expect(applyUpdate(second)).toBe(false);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
		// Another harness has its own watermark.
		expect(applyUpdate({ ...first, harnessId: "b" })).toBe(true);
	});

	it("fires the transitions of an update older than the snapshot, once, without touching the record", () => {
		const fired: string[] = [];
		const armed: string[] = [];
		phaseMirror.subscribeTransitions((id, from, to) => fired.push(`${id}:${from}>${to}`));
		phaseMirror.subscribeDelegationDeferred((id) => armed.push(id));
		// Built (and buffered) before the snapshot, applied after it.
		const buffered = phaseUpdate("a", {
			view: wireActivity("running"),
			transitions: [transitionOf("a", "spawning", "running", "x")],
			delegationArmed: true,
		});
		const snapshot = phaseUpdate("a", { view: wireActivity("waiting") });
		applySnapshot([snapshot]);
		expect(applyUpdate(buffered)).toBe(false);
		expect(fired).toEqual(["a:spawning>running"]);
		expect(armed).toEqual(["a"]);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
	});

	it("does not fire a live update's transitions again when a later snapshot covers it", () => {
		const fired: string[] = [];
		phaseMirror.subscribeTransitions((id, from, to) => fired.push(`${id}:${from}>${to}`));
		applyUpdate(
			phaseUpdate("a", {
				view: wireActivity("running"),
				transitions: [transitionOf("a", "spawning", "running", "x")],
			}),
		);
		applySnapshot([phaseUpdate("a", { view: wireActivity("running") })]);
		expect(fired).toEqual(["a:spawning>running"]);
	});

	it("keeps a forgotten harness forgotten against a stale earlier update", () => {
		const early = phaseUpdate("a", { view: wireActivity("running") });
		applyUpdate(phaseUpdate("a", { view: null }));
		applyUpdate(early);
		expect(harnessActivity.get("a")).toBeNull();
	});

	it("replaces both live lists", () => {
		applyUpdate(
			phaseUpdate("a", {
				subagents: [subagentEntry("s1")],
				backgroundTasks: [taskEntry("t1")],
			}),
		);
		expect(phaseMirror.subagents("a").map((s) => s.agentId)).toEqual(["s1"]);
		expect(phaseMirror.backgroundTasks("a").map((t) => t.taskId)).toEqual(["t1"]);
		applyUpdate(phaseUpdate("a"));
		expect(phaseMirror.subagents("a")).toEqual([]);
		expect(phaseMirror.backgroundTasks("a")).toEqual([]);
	});

	it("notifies harness listeners, then transitions in order, then deferral listeners", () => {
		const log: string[] = [];
		phaseMirror.subscribe("a", () => log.push("harness"));
		phaseMirror.subscribeSubagents("a", () => log.push("subagents"));
		phaseMirror.subscribePermissionIds(() => log.push("permission-ids"));
		phaseMirror.subscribeTransitions((id, from, to, source) =>
			log.push(`t:${id}:${from}>${to}:${source}`),
		);
		phaseMirror.subscribeDelegationDeferred((id) => log.push(`armed:${id}`));

		applyUpdate(
			phaseUpdate("a", {
				view: wireActivity("permission"),
				subagents: [subagentEntry("s")],
				transitions: [
					transitionOf("a", "spawning", "running", "x"),
					transitionOf("a", "running", "permission", "y"),
				],
				delegationArmed: true,
			}),
		);
		expect(log).toEqual([
			"harness",
			"subagents",
			"permission-ids",
			"t:a:spawning>running:x",
			"t:a:running>permission:y",
			"armed:a",
		]);
	});

	it("does not stop the other listeners when one throws", () => {
		const spy = vi.spyOn(console, "error").mockImplementation(() => {});
		const seen: string[] = [];
		phaseMirror.subscribe("a", () => {
			throw new Error("boom");
		});
		phaseMirror.subscribe("a", () => seen.push("harness"));
		phaseMirror.subscribeTransitions(() => {
			throw new Error("boom");
		});
		phaseMirror.subscribeTransitions(() => seen.push("transition"));
		phaseMirror.subscribeDelegationDeferred(() => {
			throw new Error("boom");
		});
		phaseMirror.subscribeDelegationDeferred(() => seen.push("armed"));
		applyUpdate(
			phaseUpdate("a", {
				transitions: [transitionOf("a", "running", "waiting")],
				delegationArmed: true,
			}),
		);
		expect(seen).toEqual(["harness", "transition", "armed"]);
		spy.mockRestore();
	});

	it("tracks the permission id set, replacing it only when membership changes", () => {
		const before = phaseMirror.permissionIds();
		let calls = 0;
		phaseMirror.subscribePermissionIds(() => calls++);
		applyUpdate(phaseUpdate("a", { view: wireActivity("permission") }));
		expect([...phaseMirror.permissionIds()]).toEqual(["a"]);
		expect(phaseMirror.permissionIds()).not.toBe(before);
		const held = phaseMirror.permissionIds();
		applyUpdate(phaseUpdate("b", { view: wireActivity("running") }));
		expect(phaseMirror.permissionIds()).toBe(held);
		expect(calls).toBe(1);
		applyUpdate(phaseUpdate("a", { view: wireActivity("waiting") }));
		expect([...phaseMirror.permissionIds()]).toEqual([]);
		expect(calls).toBe(2);
	});

	it("does not notify for an update that only moves lastOutputAt", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { lastOutputAt: 100 }) }));
		let calls = 0;
		phaseMirror.subscribe("a", () => calls++);
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { lastOutputAt: 900 }) }));
		expect(calls).toBe(0);
		expect(harnessActivity.get("a")?.lastOutputAt).toBe(900);
	});
});

describe("lastOutputAt", () => {
	it("recordOutput stamps the mirrored record in place, silently; nothing without a record", () => {
		harnessActivity.recordOutput("a", "chunk");
		expect(harnessActivity.get("a")).toBeNull();
		applyUpdate(phaseUpdate("a", { view: wireActivity("running") }));
		let calls = 0;
		phaseMirror.subscribe("a", () => calls++);
		const before = Date.now();
		harnessActivity.recordOutput("a", "chunk");
		expect(harnessActivity.get("a")?.lastOutputAt).toBeGreaterThanOrEqual(before);
		expect(calls).toBe(0);
	});

	it("does not stamp an exited record", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("exited", { lastOutputAt: 100 }) }));
		harnessActivity.recordOutput("a");
		expect(harnessActivity.get("a")?.lastOutputAt).toBe(100);
	});

	it("keeps the larger of the incoming and the local value", () => {
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { lastOutputAt: 100 }) }));
		harnessActivity.recordOutput("a");
		const local = harnessActivity.get("a")?.lastOutputAt ?? 0;
		applyUpdate(phaseUpdate("a", { view: wireActivity("waiting", { lastOutputAt: 200 }) }));
		expect(harnessActivity.get("a")?.lastOutputAt).toBe(local);
		const far = local + 10_000;
		applyUpdate(phaseUpdate("a", { view: wireActivity("running", { lastOutputAt: far }) }));
		expect(harnessActivity.get("a")?.lastOutputAt).toBe(far);
	});

	it("does not carry a previous spawn's local stamp into a respawn", () => {
		applyUpdate(
			phaseUpdate("a", { view: wireActivity("running", { spawnedAt: 1, lastOutputAt: 100 }) }),
		);
		harnessActivity.recordOutput("a");
		applyUpdate(
			phaseUpdate("a", {
				view: wireActivity("spawning", { spawnedAt: 2, lastOutputAt: null }),
			}),
		);
		expect(harnessActivity.get("a")?.lastOutputAt).toBeNull();
	});
});

describe("applySnapshot", () => {
	it("applies every harness, subject to the same watermark", () => {
		applySnapshot([
			phaseUpdate("a", { view: wireActivity("running") }),
			phaseUpdate("b", { view: wireActivity("waiting") }),
		]);
		expect(harnessActivity.phaseSnapshot()).toEqual({ a: "running", b: "waiting" });
	});
});

describe("startPhaseMirror", () => {
	const make = (snapshots: Array<PhaseUpdate[] | Error>) => {
		let emit: (u: PhaseUpdate) => void = () => {};
		const order: string[] = [];
		const deps: MirrorDeps = {
			listen: async (cb) => {
				order.push("listen");
				emit = cb;
				return () => {};
			},
			snapshot: async () => {
				order.push("snapshot");
				const next = snapshots.shift();
				if (next instanceof Error || next === undefined) throw next ?? new Error("none");
				return next;
			},
			delay: async () => {
				order.push("delay");
			},
			log: (m) => order.push(`log:${m.split(" ")[0]}`),
		};
		return { deps, order, emit: (u: PhaseUpdate) => emit(u) };
	};

	it("listens before the snapshot, applies the snapshot, then buffered events newer than it", async () => {
		const snap = [phaseUpdate("a", { view: wireActivity("running") })];
		const older = phaseUpdate("b", { seq: 1, view: wireActivity("running") });
		const m = make([snap]);
		// An event buffered while the snapshot is in flight.
		const newer = phaseUpdate("a", { view: wireActivity("waiting") });
		const deps: MirrorDeps = {
			...m.deps,
			snapshot: async () => {
				m.emit(older);
				m.emit(newer);
				return m.deps.snapshot();
			},
		};
		await startPhaseMirror(deps);
		expect(m.order.slice(0, 2)).toEqual(["listen", "snapshot"]);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
		// Not newer than the snapshot: dropped.
		expect(harnessActivity.get("b")).toBeNull();
	});

	it("applies live events directly once the snapshot is in", async () => {
		const m = make([[]]);
		await startPhaseMirror(m.deps);
		m.emit(phaseUpdate("a", { view: wireActivity("running") }));
		expect(harnessActivity.get("a")?.phase).toBe("running");
	});

	it("retries a failed snapshot, logs, and applies live events meanwhile", async () => {
		const snap = [phaseUpdate("a", { view: wireActivity("running") })];
		const m = make([new Error("down"), snap]);
		const started = startPhaseMirror(m.deps);
		await started;
		expect(m.order).toEqual([
			"listen",
			"snapshot",
			"log:harness_phase_snapshot",
			"delay",
			"snapshot",
		]);
		expect(harnessActivity.get("a")?.phase).toBe("running");
	});

	it("applies events that arrive during a failed snapshot", async () => {
		const m = make([new Error("down"), []]);
		const deps: MirrorDeps = {
			...m.deps,
			delay: async () => {
				m.emit(phaseUpdate("a", { view: wireActivity("running") }));
			},
		};
		await startPhaseMirror(deps);
		expect(harnessActivity.get("a")?.phase).toBe("running");
	});

	it("keeps retrying the snapshot past the fast attempts, logging once, slower after the fifth", async () => {
		const snap = [phaseUpdate("a", { view: wireActivity("waiting") })];
		const fail = Array.from({ length: 7 }, () => new Error("down"));
		const m = make([...fail, snap]);
		const delays: number[] = [];
		const deps: MirrorDeps = {
			...m.deps,
			delay: async (ms) => {
				delays.push(ms);
			},
			now: () => 0,
		};
		await startPhaseMirror(deps);
		expect(m.order.filter((o) => o === "snapshot")).toHaveLength(8);
		expect(m.order.filter((o) => o.startsWith("log:"))).toHaveLength(1);
		expect(delays).toEqual([
			...Array(SNAPSHOT_ATTEMPTS - 1).fill(SNAPSHOT_RETRY_MS),
			SNAPSHOT_SLOW_RETRY_MS,
			SNAPSHOT_SLOW_RETRY_MS,
			SNAPSHOT_SLOW_RETRY_MS,
		]);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
	});

	it("logs a repeated failure again only after the log interval", async () => {
		const m = make([new Error("1"), new Error("2"), new Error("3"), []]);
		let t = 0;
		const deps: MirrorDeps = {
			...m.deps,
			delay: async () => {
				t += FAILURE_LOG_INTERVAL_MS / 2;
			},
			now: () => t,
		};
		await startPhaseMirror(deps);
		// t = 0 logs, 30 s skipped, 60 s logs.
		expect(m.order.filter((o) => o.startsWith("log:"))).toHaveLength(2);
	});

	it("retries a failed listen, and fetches a snapshot after it succeeds", async () => {
		let listens = 0;
		let emit: (u: PhaseUpdate) => void = () => {};
		const snaps = [
			[phaseUpdate("a", { view: wireActivity("waiting") })],
			[phaseUpdate("b", { view: wireActivity("waiting") })],
		];
		const deps: MirrorDeps = {
			listen: async (cb) => {
				listens++;
				if (listens === 1) throw new Error("no bridge");
				emit = cb;
				return () => {};
			},
			snapshot: async () => snaps.shift() ?? [],
			delay: async () => {},
			log: () => {},
		};
		await startPhaseMirror(deps);
		expect(listens).toBe(2);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
		expect(harnessActivity.get("b")?.phase).toBe("waiting");
		emit(phaseUpdate("c", { view: wireActivity("running") }));
		expect(harnessActivity.get("c")?.phase).toBe("running");
	});

	it("merges a late snapshot without overwriting newer per-harness state", async () => {
		const stale = phaseUpdate("a", { view: wireActivity("running") });
		const staleOther = phaseUpdate("b", { view: wireActivity("waiting") });
		const m = make([new Error("down"), [stale, staleOther]]);
		const deps: MirrorDeps = {
			...m.deps,
			delay: async () => {
				// A live event for "a" lands before the snapshot is finally served.
				m.emit(phaseUpdate("a", { seq: stale.seq + 5, view: wireActivity("waiting") }));
			},
		};
		await startPhaseMirror(deps);
		expect(harnessActivity.get("a")?.phase).toBe("waiting");
		expect(harnessActivity.get("b")?.phase).toBe("waiting");
	});

	it("signals once when the first snapshot is applied", async () => {
		const m = make([new Error("down"), []]);
		let calls = 0;
		onFirstSnapshot(() => calls++);
		await startPhaseMirror(m.deps);
		expect(calls).toBe(1);
		applySnapshot([]);
		expect(calls).toBe(1);
	});

	it("starts only once", async () => {
		const m = make([[], []]);
		await startPhaseMirror(m.deps);
		await startPhaseMirror(m.deps);
		expect(m.order.filter((o) => o === "listen")).toHaveLength(1);
	});
});
