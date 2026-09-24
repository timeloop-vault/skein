import { beforeEach, describe, expect, it, vi } from "vitest";
import type * as ConfirmDialogModule from "./confirmDialog.ts";

// Module-level singleton store, so each test gets a fresh instance via
// vi.resetModules() + a dynamic re-import rather than an exported test
// reset hook — the module has no other reason to expose one.
let mod: typeof ConfirmDialogModule;

beforeEach(async () => {
	vi.resetModules();
	mod = await import("./confirmDialog.ts");
});

describe("confirmDialog", () => {
	it("rejects when no host is subscribed", async () => {
		await expect(mod.confirmDialog({ title: "t", message: "m" })).rejects.toThrow(/host/i);
	});

	it("resolves true when the host confirms", async () => {
		const unsubscribe = mod.subscribe(() => {});
		const result = mod.confirmDialog({ title: "t", message: "m" });
		const head = mod.getSnapshot();
		expect(head).not.toBeNull();
		mod.resolveConfirmDialog(head?.id as number, true);
		await expect(result).resolves.toBe(true);
		unsubscribe();
	});

	it("resolves false when the host cancels", async () => {
		const unsubscribe = mod.subscribe(() => {});
		const result = mod.confirmDialog({ title: "t", message: "m" });
		const head = mod.getSnapshot();
		mod.resolveConfirmDialog(head?.id as number, false);
		await expect(result).resolves.toBe(false);
		unsubscribe();
	});

	it("queues requests FIFO and only shows the head", async () => {
		const unsubscribe = mod.subscribe(() => {});
		const first = mod.confirmDialog({ title: "first", message: "m" });
		const second = mod.confirmDialog({ title: "second", message: "m" });

		// The second request is queued, not shown, while the first is head.
		expect(mod.getSnapshot()?.title).toBe("first");

		const firstId = mod.getSnapshot()?.id as number;
		mod.resolveConfirmDialog(firstId, true);
		await expect(first).resolves.toBe(true);

		// Resolving the head advances the queue to the next request.
		expect(mod.getSnapshot()?.title).toBe("second");
		const secondId = mod.getSnapshot()?.id as number;
		mod.resolveConfirmDialog(secondId, false);
		await expect(second).resolves.toBe(false);

		expect(mod.getSnapshot()).toBeNull();
		unsubscribe();
	});

	it("resolving an unknown/stale id is a no-op", async () => {
		const unsubscribe = mod.subscribe(() => {});
		const result = mod.confirmDialog({ title: "t", message: "m" });
		const head = mod.getSnapshot();
		mod.resolveConfirmDialog(999_999, true); // not the queued id
		expect(mod.getSnapshot()).toEqual(head); // still queued, unresolved
		mod.resolveConfirmDialog(head?.id as number, true);
		await expect(result).resolves.toBe(true);
		unsubscribe();
	});

	it("notifies subscribers when the snapshot changes", () => {
		const listener = vi.fn();
		const unsubscribe = mod.subscribe(listener);

		void mod.confirmDialog({ title: "t", message: "m" });
		expect(listener).toHaveBeenCalledTimes(1);

		const head = mod.getSnapshot();
		mod.resolveConfirmDialog(head?.id as number, true);
		expect(listener).toHaveBeenCalledTimes(2);

		unsubscribe();
	});

	it("stops notifying after unsubscribe", () => {
		const listener = vi.fn();
		const unsubscribe = mod.subscribe(listener);
		unsubscribe();

		// A second, still-active subscriber keeps the host "mounted" so the
		// call doesn't reject — only the unsubscribed listener is checked.
		const stillMounted = mod.subscribe(() => {});
		void mod.confirmDialog({ title: "t", message: "m" });
		expect(listener).not.toHaveBeenCalled();
		stillMounted();
	});
});
