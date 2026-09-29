import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../frontendLog.ts", () => ({
	logBoth: vi.fn(),
	logToRust: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }));

import { logBoth } from "../frontendLog.ts";
import { TRANSITION_SOURCE, harnessActivity } from "../harnessActivity.ts";
import { subagents } from "../subagents.ts";
import { forgetMailState, noteMailState } from "./mailFeed.ts";
import { __resetSupervisorForTests, startSupervisor, supervisorViolations } from "./runtime.ts";
import { RECOVERY_MIN_INTERVAL_MS } from "./supervisorState.ts";

vi.mock("../harnessEvents.ts", () => ({ liveAttach: vi.fn(() => null) }));
import { liveAttach } from "../harnessEvents.ts";

const log = vi.mocked(logBoth);
const attachMock = vi.mocked(liveAttach);
const lines = (): string[] => log.mock.calls.map((c) => c[2]);
const matching = (s: string): string[] => lines().filter((l) => l.includes(s));

let n = 0;
const spawn = (): string => {
	const id = `sv_${++n}`;
	harnessActivity.spawned(id);
	harnessActivity.attachAuthoritativeSource(id);
	harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
	return id;
};
/// The store's own paths re-grant authority when the adapter speaks, so the
/// stuck state the supervisor exists for is forged by writing the record.
const forgeLostAuthority = (id: string): void => {
	const a = harnessActivity.get(id);
	if (a) a.authoritative = false;
};
const release = (id: string): void => {
	harnessActivity.forget(id);
	subagents.forget(id);
	forgetMailState(id);
	attachMock.mockReset();
	attachMock.mockReturnValue(null);
};

describe("supervisor runtime (#423)", () => {
	let stop: () => void;
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});
	beforeEach(() => {
		log.mockClear();
		stop = startSupervisor();
	});

	const finish = (...ids: string[]): void => {
		stop();
		__resetSupervisorForTests();
		for (const id of ids) release(id);
	};

	it("is idempotent", () => {
		expect(startSupervisor()).toBe(stop);
		finish();
	});

	it("regrants authority to a speaking adapter", () => {
		const id = spawn();
		attachMock.mockReturnValue({ kind: "claude", sessionId: "s", cwd: "/x" });
		harnessActivity.detachAuthoritativeSource(id);
		vi.advanceTimersByTime(1000);
		harnessActivity.adapterDelivered(id);
		forgeLostAuthority(id);
		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		vi.advanceTimersByTime(20_000);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(matching("code=adapter_without_authority violation")).toHaveLength(1);
		expect(matching("recovery_outcome")).toHaveLength(1);
		expect(matching("recovery_outcome")[0]).toContain("applied=true");
		expect(supervisorViolations(id)).toHaveLength(1);
		finish(id);
	});

	it("moves a stale running harness to waiting with the supervisor source", () => {
		const id = spawn();
		const a = harnessActivity.get(id);
		if (!a) throw new Error("missing");
		// An end of turn the store recorded but never acted on.
		a.authoritative = false;
		a.lastTurnSignal = { kind: "end", at: Date.now() };
		const sources: string[] = [];
		const off = harnessActivity.subscribeTransitions((_i, _f, to, src) => {
			if (to === "waiting") sources.push(src);
		});
		vi.advanceTimersByTime(30_000);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(sources).toEqual([TRANSITION_SOURCE.SupervisorRecovered]);
		expect(matching("code=ended_turn_not_waiting recover recovery=")).toHaveLength(1);
		off();
		finish(id);
	});

	it("does not recover an old end of turn once the user has submitted again", () => {
		const id = spawn();
		const a = harnessActivity.get(id);
		if (!a) throw new Error("missing");
		a.authoritative = false;
		a.lastTurnSignal = { kind: "end", at: Date.now() };
		vi.advanceTimersByTime(1000);
		harnessActivity.recordInput(id, "next\r");
		vi.advanceTimersByTime(30_000);
		expect(matching("code=ended_turn_not_waiting")).toHaveLength(0);
		expect(harnessActivity.get(id)?.phase).not.toBe("waiting");
		finish(id);
	});

	it("never touches permission or exited harnesses", () => {
		const p = spawn();
		harnessActivity.detachAuthoritativeSource(p);
		harnessActivity.setWaitingFromAdapter(p, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		harnessActivity.setPermissionFromAdapter(p, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		const e = spawn();
		harnessActivity.detachAuthoritativeSource(e);
		harnessActivity.setWaitingFromAdapter(e, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		harnessActivity.exited(e, 0);
		vi.advanceTimersByTime(30_000);
		expect(harnessActivity.get(p)?.phase).toBe("permission");
		expect(harnessActivity.get(e)?.phase).toBe("exited");
		expect(matching("code=ended_turn_not_waiting")).toHaveLength(0);
		finish(p, e);
	});

	it("caps recovery attempts at three, then reports exhaustion once", () => {
		const id = spawn();
		attachMock.mockReturnValue({ kind: "claude", sessionId: "s", cwd: "/x" });
		// A recurring condition: authority is lost again after each repair.
		for (let i = 0; i < 8; i++) {
			harnessActivity.detachAuthoritativeSource(id);
			vi.advanceTimersByTime(1000);
			harnessActivity.adapterDelivered(id);
			forgeLostAuthority(id);
			vi.advanceTimersByTime(RECOVERY_MIN_INTERVAL_MS + 2000);
		}
		expect(matching("code=adapter_without_authority recover recovery=")).toHaveLength(3);
		expect(matching("recovery_exhausted")).toHaveLength(1);
		finish(id);
	});

	it("only logs a detect-only invariant", () => {
		const id = spawn();
		harnessActivity.setWaitingFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		noteMailState(id, { unread: 2 });
		vi.advanceTimersByTime(90_000);
		expect(matching("code=mail_held violation")).toHaveLength(1);
		expect(harnessActivity.get(id)?.phase).toBe("waiting");
		expect(matching(" recover recovery=")).toHaveLength(0);
		finish(id);
	});
});
