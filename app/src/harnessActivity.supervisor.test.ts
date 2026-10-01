import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { harnessActivity, onTick, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { TICK_INTERVAL_MS } from "./harnessActivityConstants.ts";

// #423 — the facts the harness supervisor reads, and its two guarded
// recovery methods. Own file: the tick is a module-global `setInterval`
// started by the first `spawned()`, so fake timers go in first (same
// convention as `harnessActivity.delegation.test.ts`).

const nextId = (() => {
	let n = 0;
	return () => `s_${++n}`;
})();

describe("supervisor facts and recovery (#423)", () => {
	beforeAll(() => {
		vi.useFakeTimers();
	});
	afterAll(() => {
		vi.useRealTimers();
	});

	it("records lastTurnSignal even when the harness is not authoritative", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		expect(harnessActivity.get(id)?.lastTurnSignal).toBeNull();
		vi.setSystemTime(1_000);
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		expect(harnessActivity.get(id)?.lastTurnSignal).toEqual({ kind: "work", at: 1_000 });
		vi.setSystemTime(2_000);
		harnessActivity.awaitingPromptFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeEndTurn);
		expect(harnessActivity.get(id)?.lastTurnSignal).toEqual({ kind: "end", at: 2_000 });
		harnessActivity.forget(id);
	});

	it("records a work signal that permission then ignores", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		vi.setSystemTime(3_000);
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		expect(harnessActivity.get(id)?.phase).toBe("permission");
		expect(harnessActivity.get(id)?.lastTurnSignal).toEqual({ kind: "work", at: 3_000 });
		harnessActivity.forget(id);
	});

	it("stamps authorityLostAt on detach, phaseSince on a real change, permissionAt while in permission", () => {
		const id = nextId();
		vi.setSystemTime(10_000);
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		expect(harnessActivity.get(id)?.authorityLostAt).toBeNull();
		vi.setSystemTime(11_000);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		expect(harnessActivity.get(id)?.phaseSince).toBe(11_000);
		expect(harnessActivity.get(id)?.permissionAt).toBe(11_000);
		vi.setSystemTime(12_000);
		harnessActivity.setPermissionFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudePermission, "Bash");
		expect(harnessActivity.get(id)?.phaseSince).toBe(11_000);
		expect(harnessActivity.get(id)?.permissionAt).toBe(12_000);
		harnessActivity.releasePermission(id, TRANSITION_SOURCE.AdapterDetached);
		expect(harnessActivity.get(id)?.permissionAt).toBeNull();
		vi.setSystemTime(13_000);
		harnessActivity.detachAuthoritativeSource(id);
		expect(harnessActivity.get(id)?.authorityLostAt).toBe(13_000);
		harnessActivity.forget(id);
	});

	it("stamps lastAdapterEvent on every adapterDelivered call, including no-ops", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		vi.setSystemTime(20_000);
		harnessActivity.adapterDelivered(id);
		vi.setSystemTime(21_000);
		harnessActivity.adapterDelivered(id, { restoresAuthority: false });
		expect(harnessActivity.get(id)?.lastAdapterEvent).toEqual({
			at: 21_000,
			restoresAuthority: false,
		});
		harnessActivity.forget(id);
	});

	it("supervisorRegrantAuthority applies only when authority is lost and nothing else explains it", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		expect(harnessActivity.supervisorRegrantAuthority(id)).toBe(false); // already authoritative
		harnessActivity.detachAuthoritativeSource(id);
		expect(harnessActivity.supervisorRegrantAuthority(id)).toBe(true);
		expect(harnessActivity.get(id)?.authoritative).toBe(true);
		expect(harnessActivity.supervisorRegrantAuthority("nope")).toBe(false);

		harnessActivity.detachAuthoritativeSource(id);
		harnessActivity.exited(id, 0);
		expect(harnessActivity.supervisorRegrantAuthority(id)).toBe(false);
		expect(harnessActivity.get(id)?.authoritative).toBe(false);
		harnessActivity.forget(id);
	});

	it("supervisorRegrantAuthority refuses while a watchdog diagnosis stands", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.attachAuthoritativeSource(id);
		harnessActivity.recordInput(id, "\r");
		vi.advanceTimersByTime(60_000);
		expect(harnessActivity.get(id)?.adapterSilent).toBe(true);
		expect(harnessActivity.get(id)?.authorityLostAt).not.toBeNull();
		expect(harnessActivity.supervisorRegrantAuthority(id)).toBe(false);
		harnessActivity.forget(id);
	});

	it("supervisorSetWaiting moves running and idle, and nothing else", () => {
		const running = nextId();
		harnessActivity.spawned(running);
		harnessActivity.setRunningFromAdapter(running, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		expect(harnessActivity.supervisorSetWaiting(running)).toBe(true);
		expect(harnessActivity.get(running)?.phase).toBe("waiting");
		expect(harnessActivity.supervisorSetWaiting(running)).toBe(false); // waiting

		const spawning = nextId();
		harnessActivity.spawned(spawning);
		expect(harnessActivity.supervisorSetWaiting(spawning)).toBe(false);

		const permission = nextId();
		harnessActivity.spawned(permission);
		harnessActivity.setPermissionFromAdapter(
			permission,
			TRANSITION_SOURCE.L2c1ClaudePermission,
			null,
		);
		expect(harnessActivity.supervisorSetWaiting(permission)).toBe(false);
		expect(harnessActivity.get(permission)?.phase).toBe("permission");

		const exited = nextId();
		harnessActivity.spawned(exited);
		harnessActivity.exited(exited, 0);
		expect(harnessActivity.supervisorSetWaiting(exited)).toBe(false);
		expect(harnessActivity.get(exited)?.phase).toBe("exited");

		for (const id of [running, spawning, permission, exited]) harnessActivity.forget(id);
	});

	it("supervisorSetWaiting reports its own transition source", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		harnessActivity.setRunningFromAdapter(id, TRANSITION_SOURCE.L2c1ClaudeAssistant);
		const seen: string[] = [];
		const off = harnessActivity.subscribeTransitions((_id, _from, _to, source) => {
			seen.push(source);
		});
		harnessActivity.supervisorSetWaiting(id);
		off();
		expect(seen).toEqual([TRANSITION_SOURCE.SupervisorRecovered]);
		harnessActivity.forget(id);
	});

	it("onTick calls listeners with the tick's now, survives a throwing listener, and unsubscribes", () => {
		const id = nextId();
		harnessActivity.spawned(id);
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		const seen: number[] = [];
		const offBad = onTick(() => {
			throw new Error("boom");
		});
		const off = onTick((now) => seen.push(now));
		vi.setSystemTime(100_000);
		vi.advanceTimersByTime(TICK_INTERVAL_MS);
		expect(seen).toEqual([100_000 + TICK_INTERVAL_MS]);
		expect(err).toHaveBeenCalled();
		off();
		offBad();
		vi.advanceTimersByTime(TICK_INTERVAL_MS);
		expect(seen).toHaveLength(1);
		err.mockRestore();
		harnessActivity.forget(id);
	});
});
