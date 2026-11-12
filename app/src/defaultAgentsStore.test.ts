import { describe, expect, it } from "vitest";
import {
	LEGACY_DEFAULT_AGENTS_KEY,
	loadDefaultAgents,
	setDefaultAgent,
} from "./defaultAgentsStore.ts";

type Call = { cmd: string; args: Record<string, unknown> | undefined };

const harness = (stored: string | null, reply: unknown) => {
	const calls: Call[] = [];
	const reads: string[] = [];
	const deps = {
		invoke: (async (cmd: string, args?: Record<string, unknown>) => {
			calls.push({ cmd, args });
			return reply;
		}) as <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
		readStorage: (key: string) => {
			reads.push(key);
			return stored;
		},
	};
	return { calls, reads, deps };
};

describe("loadDefaultAgents (#730)", () => {
	it("sends the legacy value to the import and returns the daemon's map", async () => {
		const h = harness(JSON.stringify({ claude: "coder" }), {
			imported: true,
			defaultAgents: { claude: "coder", opencode: "plan" },
		});
		expect(await loadDefaultAgents(h.deps)).toEqual({ claude: "coder", opencode: "plan" });
		expect(h.calls).toEqual([
			{ cmd: "default_agents_import", args: { defaultAgents: { claude: "coder" } } },
		]);
		expect(h.reads).toEqual([LEGACY_DEFAULT_AGENTS_KEY]);
	});

	it("imports an empty map when the legacy key is absent", async () => {
		const h = harness(null, { imported: true, defaultAgents: {} });
		expect(await loadDefaultAgents(h.deps)).toEqual({});
		expect(h.calls[0]?.args).toEqual({ defaultAgents: {} });
	});

	it("imports an empty map when the legacy value is garbage", async () => {
		for (const junk of ["{not json", "null", "[1]", '"x"']) {
			const h = harness(junk, { imported: false, defaultAgents: {} });
			await loadDefaultAgents(h.deps);
			expect(h.calls[0]?.args).toEqual({ defaultAgents: {} });
		}
	});

	it("does not overwrite: the daemon's existing map wins over the legacy value", async () => {
		const h = harness(JSON.stringify({ claude: "old" }), {
			imported: false,
			defaultAgents: { claude: "new" },
		});
		expect(await loadDefaultAgents(h.deps)).toEqual({ claude: "new" });
	});

	it("never writes or removes the legacy key", async () => {
		const writes: string[] = [];
		const orig = globalThis.localStorage;
		const fake = {
			getItem: () => null,
			setItem: (k: string) => writes.push(k),
			removeItem: (k: string) => writes.push(k),
		};
		Object.defineProperty(globalThis, "localStorage", { value: fake, configurable: true });
		try {
			const h = harness("{}", { imported: false, defaultAgents: {} });
			await loadDefaultAgents(h.deps);
			await setDefaultAgent(h.deps, "claude", "x");
		} finally {
			Object.defineProperty(globalThis, "localStorage", { value: orig, configurable: true });
		}
		expect(writes).toEqual([]);
	});
});

describe("setDefaultAgent (#730)", () => {
	it("passes the name and returns the new map", async () => {
		const h = harness(null, { claude: "coder" });
		expect(await setDefaultAgent(h.deps, "claude", "coder")).toEqual({ claude: "coder" });
		expect(h.calls).toEqual([
			{ cmd: "default_agent_set", args: { kind: "claude", agent: "coder" } },
		]);
	});

	it("sends null to clear", async () => {
		const h = harness(null, {});
		await setDefaultAgent(h.deps, "claude", undefined);
		expect(h.calls[0]?.args).toEqual({ kind: "claude", agent: null });
	});
});
