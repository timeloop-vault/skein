import { afterEach, describe, expect, it, vi } from "vitest";
import {
	type ClaimInvoke,
	hermesPollDelay,
	isHermesProgram as isHermes,
	markClaimed,
	startHermesCapture,
} from "./hermesCapture.ts";

type Args = Parameters<ClaimInvoke>[1];

const flush = async () => {
	for (let i = 0; i < 5; i++) await Promise.resolve();
};

const stops: (() => void)[] = [];
const start = (p: Parameters<typeof startHermesCapture>[0]) => {
	const stop = startHermesCapture(p);
	stops.push(stop);
	return stop;
};

afterEach(() => {
	for (const s of stops.splice(0)) s();
	vi.useRealTimers();
});

describe("hermesCapture", () => {
	it("captures once and stops polling", async () => {
		vi.useFakeTimers();
		const calls: Args[] = [];
		const invoke: ClaimInvoke = async (_c, args) => {
			calls.push(args);
			return calls.length === 2 ? "20260101_000000_aaaaaa" : null;
		};
		const onCapture = vi.fn();
		start({ harnessId: "h1", cwd: "/r", spawnedAtMs: 1000, onCapture, invoke });
		await vi.advanceTimersByTimeAsync(5_000);
		expect(onCapture).toHaveBeenCalledTimes(1);
		expect(onCapture).toHaveBeenCalledWith("20260101_000000_aaaaaa");
		expect(calls).toHaveLength(2);
	});

	it("skips an id that became claimed during the await", async () => {
		vi.useFakeTimers();
		let release: (v: string | null) => void = () => {};
		const invoke: ClaimInvoke = () =>
			new Promise((r) => {
				release = r;
			});
		const onCapture = vi.fn();
		start({ harnessId: "h2", cwd: "/r2", spawnedAtMs: 1, onCapture, invoke });
		await vi.advanceTimersByTimeAsync(1_000);
		markClaimed("20260101_000000_race01");
		release("20260101_000000_race01");
		await flush();
		expect(onCapture).not.toHaveBeenCalled();
	});

	it("two captures in one cwd pass both pending entries and get their own ids", async () => {
		vi.useFakeTimers();
		const seen: Record<string, Args> = {};
		const ids: Record<string, string> = {
			a: "20260101_000000_idaaaa",
			b: "20260101_000001_idbbbb",
		};
		const invoke: ClaimInvoke = async (_c, args) => {
			seen[args.harnessId] = args;
			return ids[args.harnessId] ?? null;
		};
		const got: Record<string, string> = {};
		start({ harnessId: "a", cwd: "/two", spawnedAtMs: 10, onCapture: (i) => (got.a = i), invoke });
		start({ harnessId: "b", cwd: "/two", spawnedAtMs: 20, onCapture: (i) => (got.b = i), invoke });
		await vi.advanceTimersByTimeAsync(1_000);
		expect(got).toEqual(ids);
		expect(seen.a?.pending.map((p) => p.harnessId).sort()).toEqual(["a", "b"]);
		expect(seen.b?.pending.map((p) => p.harnessId)).toContain("b");
	});

	it("stop() before capture ends polling and removes the pending entry", async () => {
		vi.useFakeTimers();
		const calls: Args[] = [];
		const invoke: ClaimInvoke = async (_c, args) => {
			calls.push(args);
			return null;
		};
		const stop = start({
			harnessId: "s1",
			cwd: "/stop",
			spawnedAtMs: 1,
			onCapture: vi.fn(),
			invoke,
		});
		start({ harnessId: "s2", cwd: "/stop", spawnedAtMs: 2, onCapture: vi.fn(), invoke });
		stop();
		stop();
		await vi.advanceTimersByTimeAsync(3_000);
		expect(calls.length).toBeGreaterThan(0);
		for (const c of calls) {
			expect(c.harnessId).toBe("s2");
			expect(c.pending.map((p) => p.harnessId)).toEqual(["s2"]);
		}
	});

	it("retries after a rejected invoke", async () => {
		vi.useFakeTimers();
		let n = 0;
		const invoke: ClaimInvoke = async () => {
			n++;
			if (n === 1) throw new Error("database is locked");
			return "20260101_000000_retry1";
		};
		const onCapture = vi.fn();
		start({ harnessId: "r1", cwd: "/retry", spawnedAtMs: 1, onCapture, invoke });
		await vi.advanceTimersByTimeAsync(3_000);
		expect(onCapture).toHaveBeenCalledWith("20260101_000000_retry1");
	});

	it("sends a bound harness's markClaimed id in claimed", async () => {
		vi.useFakeTimers();
		markClaimed("20260101_000000_bound1");
		const calls: Args[] = [];
		const invoke: ClaimInvoke = async (_c, args) => {
			calls.push(args);
			return null;
		};
		start({ harnessId: "c1", cwd: "/claimed", spawnedAtMs: 1, onCapture: vi.fn(), invoke });
		await vi.advanceTimersByTimeAsync(1_000);
		expect(calls[0]?.claimed).toContain("20260101_000000_bound1");
	});

	it("backs off from 1 s to 3 s after two minutes", () => {
		expect(hermesPollDelay(0)).toBe(1_000);
		expect(hermesPollDelay(119_999)).toBe(1_000);
		expect(hermesPollDelay(120_000)).toBe(3_000);
	});
});

describe("isHermesProgram", () => {
	it("recognises hermes by bare name, extension and absolute path on either OS", () => {
		for (const p of [
			"hermes",
			"hermes.exe",
			"C:\\Users\\x\\hermes.cmd",
			"/usr/local/bin/hermes",
			"HERMES.EXE",
		]) {
			expect(isHermes(p)).toBe(true);
		}
		for (const p of [
			undefined,
			"",
			"pwsh.exe",
			"C:\\Windows\\System32\\cmd.exe",
			"/bin/zsh",
			"hermes-agent",
		]) {
			expect(isHermes(p)).toBe(false);
		}
	});
});

describe("hermesCapture profile (#627)", () => {
	it("sends the profile and only pends same-cwd same-profile harnesses", async () => {
		vi.useFakeTimers();
		const calls: Args[] = [];
		const invoke: ClaimInvoke = async (_c, args) => {
			calls.push(args);
			return null;
		};
		const onCapture = vi.fn();
		start({ harnessId: "pa", cwd: "/p", spawnedAtMs: 1, profile: "skein", onCapture, invoke });
		start({ harnessId: "pb", cwd: "/p", spawnedAtMs: 2, profile: "work", onCapture, invoke });
		start({ harnessId: "pc", cwd: "/p", spawnedAtMs: 3, onCapture, invoke });
		await vi.advanceTimersByTimeAsync(1_100);
		const byId = (id: string) => calls.find((c) => c.harnessId === id);
		expect(byId("pa")?.profile).toBe("skein");
		expect(byId("pa")?.pending.map((x) => x.harnessId)).toEqual(["pa"]);
		expect(byId("pb")?.profile).toBe("work");
		expect(byId("pc")?.profile).toBeNull();
	});
});
