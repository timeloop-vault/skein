import { describe, expect, it, vi } from "vitest";
import type { GateResult } from "./harnessInputGate.ts";
import { createDropGate } from "./terminalDrop.ts";

const flush = async () => {
	for (let i = 0; i < 5; i++) await Promise.resolve();
};

const setup = (local: GateResult | null, answer: Promise<GateResult>) => {
	const set = vi.fn();
	const ask = vi.fn(() => answer);
	const gate = createDropGate({ local: () => local, ask, set });
	return { set, ask, gate };
};

describe("createDropGate", () => {
	it("shows the local OK at once, then the daemon's refusal", async () => {
		const refusal: GateResult = { ok: false, reason: "dialog open", code: "permission" };
		const { set, ask, gate } = setup(null, Promise.resolve(refusal));
		gate.hover();
		expect(set).toHaveBeenLastCalledWith({ ok: true });
		await flush();
		expect(set).toHaveBeenLastCalledWith(refusal);
		expect(ask).toHaveBeenCalledTimes(1);
	});

	it("asks the daemon once per drag, not per over event", async () => {
		const { ask, gate } = setup(null, Promise.resolve({ ok: true }));
		gate.hover();
		gate.hover();
		gate.hover();
		await flush();
		expect(ask).toHaveBeenCalledTimes(1);
	});

	it("a local refusal short-circuits: no daemon call", async () => {
		const refusal: GateResult = { ok: false, reason: "not ready" };
		const { set, ask, gate } = setup(refusal, Promise.resolve({ ok: true }));
		gate.hover();
		await flush();
		expect(set).toHaveBeenCalledWith(refusal);
		expect(ask).not.toHaveBeenCalled();
	});

	it("ignores a late answer after the drag ended", async () => {
		let resolve: (r: GateResult) => void = () => {};
		const { set, gate } = setup(
			null,
			new Promise<GateResult>((r) => {
				resolve = r;
			}),
		);
		gate.hover();
		gate.end();
		expect(set).toHaveBeenLastCalledWith(null);
		resolve({ ok: false, reason: "late" });
		await flush();
		expect(set).toHaveBeenLastCalledWith(null);
		expect(set).toHaveBeenCalledTimes(2);
	});

	it("ignores an answer from a previous drag after a new one starts", async () => {
		const resolvers: Array<(r: GateResult) => void> = [];
		const set = vi.fn();
		const gate = createDropGate({
			local: () => null,
			ask: () => new Promise<GateResult>((r) => resolvers.push(r)),
			set,
		});
		gate.hover();
		gate.end();
		gate.hover();
		resolvers[0]?.({ ok: false, reason: "old" });
		await flush();
		expect(set).toHaveBeenLastCalledWith({ ok: true });
	});
});
