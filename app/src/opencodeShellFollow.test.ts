import { describe, expect, it } from "vitest";
import {
	createPortWait,
	createScanScheduler,
	createStartupProof,
	DEBOUNCE_MS,
	decideShellRebind,
	isOpencodeShellSpawn,
	MIN_GAP_MS,
	type OpencodeScan,
	PORT_WAIT_MS,
	RECHECK_MS,
	type ScanResult,
	SETTLED_GAP_MS,
	STARTUP_PROOF_MS,
	shellFollowHint,
} from "./opencodeShellFollow.ts";

const scan = (o: Partial<OpencodeScan> = {}): OpencodeScan => ({
	pid: 1,
	sessionId: null,
	port: null,
	portConfirmed: false,
	continueLast: false,
	...o,
});

describe("isOpencodeShellSpawn", () => {
	it("is shell mode for opencode panes not running opencode", () => {
		expect(isOpencodeShellSpawn("opencode", "/bin/zsh")).toBe(true);
		expect(isOpencodeShellSpawn("opencode", "pwsh.exe")).toBe(true);
		expect(isOpencodeShellSpawn("opencode", "opencode")).toBe(false);
		expect(isOpencodeShellSpawn("opencode", "C:\\x\\opencode.exe")).toBe(false);
		expect(isOpencodeShellSpawn("claude", "/bin/zsh")).toBe(false);
		expect(isOpencodeShellSpawn("opencode", undefined)).toBe(false);
	});
});

describe("decideShellRebind", () => {
	const cur = { sessionId: "ses_a", port: 4000 };
	it("returns null when nothing differs", () => {
		expect(decideShellRebind(scan({ sessionId: "ses_a" }), cur)).toBeNull();
		expect(decideShellRebind(scan(), cur)).toBeNull();
	});
	it("re-binds a differing session id", () => {
		expect(decideShellRebind(scan({ sessionId: "ses_b" }), cur)).toEqual({ sessionId: "ses_b" });
	});
	it("re-points only a confirmed, different port", () => {
		expect(decideShellRebind(scan({ port: 5000 }), cur)).toBeNull();
		expect(decideShellRebind(scan({ port: 5000, portConfirmed: true }), cur)).toEqual({
			port: 5000,
		});
		expect(decideShellRebind(scan({ port: 4000, portConfirmed: true }), cur)).toBeNull();
	});
	it("does both at once", () => {
		expect(
			decideShellRebind(scan({ sessionId: "ses_b", port: 5000, portConfirmed: true }), {
				sessionId: undefined,
				port: undefined,
			}),
		).toEqual({ sessionId: "ses_b", port: 5000 });
	});
});

const R = (again: boolean, settledPid: number | null = null): ScanResult => ({
	again,
	settledPid,
});

function harness(results: ScanResult[]) {
	let t = 0;
	const timers: { at: number; fn: () => void; id: number }[] = [];
	let nid = 0;
	const starts: number[] = [];
	const s = createScanScheduler({
		now: () => t,
		setTimer: (fn, ms) => {
			const id = nid++;
			timers.push({ at: t + ms, fn, id });
			return id;
		},
		clearTimer: (id) => {
			const i = timers.findIndex((x) => x.id === id);
			if (i >= 0) timers.splice(i, 1);
		},
		scan: async () => {
			starts.push(t);
			return results.shift() ?? { again: false, settledPid: null };
		},
	});
	const advance = async (ms: number) => {
		const end = t + ms;
		for (;;) {
			timers.sort((a, b) => a.at - b.at);
			const next = timers[0];
			if (!next || next.at > end) break;
			timers.shift();
			t = next.at;
			next.fn();
			await Promise.resolve();
			await Promise.resolve();
		}
		t = end;
	};
	return { s, advance, starts, timers };
}

describe("createScanScheduler", () => {
	it("coalesces a burst of ticks into one trailing scan", async () => {
		const h = harness([R(false)]);
		for (let i = 0; i < 20; i++) {
			h.s.tick();
			await h.advance(50);
		}
		await h.advance(DEBOUNCE_MS);
		expect(h.starts).toHaveLength(1);
	});
	it("keeps a minimum gap between scans", async () => {
		const h = harness([R(false), R(false)]);
		h.s.tick();
		await h.advance(DEBOUNCE_MS);
		h.s.tick();
		await h.advance(DEBOUNCE_MS);
		expect(h.starts).toHaveLength(1);
		await h.advance(MIN_GAP_MS);
		expect(h.starts).toHaveLength(2);
		expect((h.starts[1] ?? 0) - (h.starts[0] ?? 0)).toBeGreaterThanOrEqual(MIN_GAP_MS);
	});
	it("re-checks once after a scan asks for it, and then stops", async () => {
		const h = harness([R(true), R(true)]);
		h.s.tick();
		await h.advance(DEBOUNCE_MS + RECHECK_MS + MIN_GAP_MS);
		expect(h.starts).toHaveLength(2);
		await h.advance(60_000);
		expect(h.starts).toHaveLength(2);
	});
	it("widens the gap while the same pid stays settled, and resets on null", async () => {
		const h = harness([R(false, 7), R(false, 7), R(false), R(false)]);
		h.s.tick();
		await h.advance(DEBOUNCE_MS);
		expect(h.starts).toHaveLength(1);
		h.s.tick();
		await h.advance(SETTLED_GAP_MS - 100);
		expect(h.starts).toHaveLength(1);
		await h.advance(200);
		expect(h.starts).toHaveLength(2);
		// Scan 3 still waits the long gap (scan 2 settled pid 7) and returns
		// nothing settled, so scan 4 needs only the short one.
		h.s.tick();
		await h.advance(SETTLED_GAP_MS + 100);
		expect(h.starts).toHaveLength(3);
		h.s.tick();
		await h.advance(MIN_GAP_MS + DEBOUNCE_MS);
		expect(h.starts).toHaveLength(4);
	});
	it("does not widen the gap for an unsettled pid", async () => {
		const h = harness([R(false, null), R(false, null)]);
		h.s.tick();
		await h.advance(DEBOUNCE_MS);
		h.s.tick();
		await h.advance(DEBOUNCE_MS + MIN_GAP_MS);
		expect(h.starts).toHaveLength(2);
	});
	it("schedules nothing while quiet and nothing after dispose", async () => {
		const h = harness([R(false)]);
		await h.advance(60_000);
		expect(h.starts).toHaveLength(0);
		h.s.tick();
		h.s.dispose();
		await h.advance(60_000);
		expect(h.starts).toHaveLength(0);
		expect(h.timers).toHaveLength(0);
	});
});

describe("shellFollowHint", () => {
	it("words each case", () => {
		expect(shellFollowHint(true, true)).toContain("follows its session");
		expect(shellFollowHint(false, true)).toContain("can't see its activity");
		expect(shellFollowHint(false, false)).toContain("can't follow");
	});
	it("names a continued session it cannot identify", () => {
		expect(shellFollowHint(false, false, true)).toContain("which session it continued");
		expect(shellFollowHint(false, true, true)).toContain("can't see its activity");
		expect(shellFollowHint(true, true, true)).toContain("follows its session");
	});
});

describe("createPortWait", () => {
	it("waits for PORT_WAIT_MS per pid, then gives up", () => {
		let t = 0;
		const w = createPortWait(() => t);
		expect(w.waiting(1)).toBe(true);
		t = PORT_WAIT_MS - 1;
		expect(w.waiting(1)).toBe(true);
		expect(w.waiting(2)).toBe(true);
		t = PORT_WAIT_MS;
		expect(w.waiting(1)).toBe(false);
		expect(w.waiting(2)).toBe(true);
		w.clear();
		expect(w.waiting(1)).toBe(true);
	});
});

describe("retry chain", () => {
	it("keeps re-checking on retry results without further ticks, until one stops", async () => {
		const h = harness([
			{ again: true, retry: true, settledPid: null },
			{ again: true, retry: true, settledPid: null },
			{ again: true, retry: true, settledPid: null },
			R(false, 7),
		]);
		h.s.tick();
		await h.advance(DEBOUNCE_MS + 10 * RECHECK_MS);
		expect(h.starts).toHaveLength(4);
		await h.advance(60_000);
		expect(h.starts).toHaveLength(4);
	});
});

describe("createStartupProof", () => {
	const mk = () => {
		let t = 0;
		return {
			p: createStartupProof(() => t),
			at: (ms: number) => {
				t = ms;
			},
		};
	};
	it("does not trust a bogus id seen once then gone", () => {
		const { p, at } = mk();
		expect(p.proven(scan({ pid: 5, sessionId: "ses_typo", port: 4000 }))).toBe(false);
		at(1500);
		p.clear(); // the next scan returned null
		at(9000);
		expect(p.proven(scan({ pid: 6, sessionId: "ses_x", port: 4000 }))).toBe(false);
	});
	it("trusts the same pid seen again after the proof window", () => {
		const { p, at } = mk();
		const s = scan({ pid: 5, sessionId: "ses_ok", port: 4000 });
		expect(p.proven(s)).toBe(false);
		at(STARTUP_PROOF_MS - 1);
		expect(p.proven(s)).toBe(false);
		at(STARTUP_PROOF_MS);
		expect(p.proven(s)).toBe(true);
	});
	it("trusts a confirmed port immediately", () => {
		const { p } = mk();
		expect(p.proven(scan({ pid: 5, port: 4000, portConfirmed: true }))).toBe(true);
	});
});
