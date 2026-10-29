import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
const confirmMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("./confirmDialog.ts", () => ({ confirmDialog: confirmMock }));

import { confirmDiscardUnsaved, restartApp } from "./exitGuard.ts";
import { filesRegistry } from "./filesRegistry.ts";

let unregister: (() => void) | undefined;
const dirty = (...names: string[]) => {
	unregister = filesRegistry.register("h-files", { dirtyNames: () => names });
};

beforeEach(() => {
	invokeMock.mockReset().mockResolvedValue(undefined);
	confirmMock.mockReset();
});
afterEach(() => unregister?.());

describe("restartApp (#692)", () => {
	it("restarts without asking when nothing is unsaved", async () => {
		expect(await restartApp()).toBe(true);
		expect(confirmMock).not.toHaveBeenCalled();
		expect(invokeMock).toHaveBeenCalledWith("app_restart");
	});

	it("keeps Skein running when the user cancels the unsaved-files prompt", async () => {
		dirty("a.ts");
		confirmMock.mockResolvedValue(false);
		expect(await restartApp()).toBe(false);
		expect(invokeMock).not.toHaveBeenCalled();
	});

	it("restarts once the user accepts discarding, and says restart, not quit", async () => {
		dirty("a.ts", "b.ts");
		confirmMock.mockResolvedValue(true);
		expect(await restartApp()).toBe(true);
		expect(confirmMock).toHaveBeenCalledWith(
			expect.objectContaining({
				confirmLabel: "Restart anyway",
				message: "2 unsaved files (a.ts, b.ts) will be discarded. Restart anyway?",
			}),
		);
		expect(invokeMock).toHaveBeenCalledWith("app_restart");
	});

	it("fails closed: a prompt that cannot be shown restarts nothing", async () => {
		dirty("a.ts");
		confirmMock.mockRejectedValue(new Error("no dialog host"));
		await expect(restartApp()).rejects.toThrow("no dialog host");
		expect(invokeMock).not.toHaveBeenCalled();
	});
});

describe("confirmDiscardUnsaved", () => {
	it("keeps quit's wording", async () => {
		dirty("a.ts");
		confirmMock.mockResolvedValue(true);
		expect(await confirmDiscardUnsaved("Quit")).toBe(true);
		expect(confirmMock).toHaveBeenCalledWith(
			expect.objectContaining({
				confirmLabel: "Quit anyway",
				message: "1 unsaved file (a.ts) will be discarded. Quit anyway?",
			}),
		);
	});

	it("does not stack a second prompt while one is open", async () => {
		dirty("a.ts");
		let answer: (ok: boolean) => void = () => {};
		confirmMock.mockReturnValue(new Promise<boolean>((resolve) => (answer = resolve)));
		const first = confirmDiscardUnsaved("Quit");
		expect(await confirmDiscardUnsaved("Quit")).toBe(false);
		answer(true);
		expect(await first).toBe(true);
		expect(confirmMock).toHaveBeenCalledTimes(1);
	});
});
