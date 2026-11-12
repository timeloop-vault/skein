import { describe, expect, it, vi } from "vitest";
import type { CreateRoomArgs } from "./NewRoomDialogTypes.ts";
import { submitNewRoom } from "./newRoomSubmit.ts";
import type { CreateRoomOutcome } from "./worktreeRoom.ts";

const args = { cwd: "/r" } as unknown as CreateRoomArgs;
const okOutcome: CreateRoomOutcome = {
	ok: true,
	args,
	remember: { baseBranch: "main", branchTemplate: "t/" },
};

const setup = (over: {
	prepare?: () => Promise<CreateRoomOutcome>;
	commit?: () => Promise<{ ok: true } | { ok: false; error: string }>;
}) => {
	const deps = {
		prepare: over.prepare ?? (async () => okOutcome),
		commit: vi.fn(over.commit ?? (async () => ({ ok: true as const }))),
		remember: vi.fn(),
		setError: vi.fn(),
		setBusy: vi.fn(),
	};
	return deps;
};

describe("submitNewRoom", () => {
	it("remembers the folder on success and leaves no error", async () => {
		const d = setup({});
		await submitNewRoom(d);
		expect(d.commit).toHaveBeenCalledWith(args);
		expect(d.remember).toHaveBeenCalledWith("main", "t/");
		expect(d.setError).toHaveBeenCalledTimes(1);
		expect(d.setError).toHaveBeenCalledWith(null);
		expect(d.setBusy).toHaveBeenCalledTimes(1);
		expect(d.setBusy).toHaveBeenCalledWith(true);
	});

	it("sets the error, clears busy and does not remember when the commit fails", async () => {
		const d = setup({ commit: async () => ({ ok: false, error: "refused" }) });
		await submitNewRoom(d);
		expect(d.setError).toHaveBeenLastCalledWith("refused");
		expect(d.setBusy).toHaveBeenLastCalledWith(false);
		expect(d.remember).not.toHaveBeenCalled();
	});

	it("does not commit or remember when preparing fails validation", async () => {
		const d = setup({ prepare: async () => ({ ok: false, error: "bad branch" }) });
		await submitNewRoom(d);
		expect(d.setError).toHaveBeenLastCalledWith("bad branch");
		expect(d.setBusy).toHaveBeenLastCalledWith(false);
		expect(d.commit).not.toHaveBeenCalled();
		expect(d.remember).not.toHaveBeenCalled();
	});

	it("turns a thrown error into the error text and clears busy", async () => {
		const d = setup({
			commit: async () => {
				throw new Error("boom");
			},
		});
		await submitNewRoom(d);
		expect(d.setError).toHaveBeenLastCalledWith("boom");
		expect(d.setBusy).toHaveBeenLastCalledWith(false);
		expect(d.remember).not.toHaveBeenCalled();
	});
});
