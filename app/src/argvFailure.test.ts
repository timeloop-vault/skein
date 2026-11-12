import { describe, expect, it } from "vitest";
import {
	ArgvPrepareError,
	argvFailureToast,
	prepareOrThrow,
	reportArgvFailure,
	setArgvFailureSink,
} from "./argvFailure.ts";
import type { NewToast } from "./toastStack.ts";

describe("argvFailure (#730)", () => {
	it("prepareOrThrow rethrows a rejection as an ArgvPrepareError", async () => {
		await expect(
			prepareOrThrow("x", async () => {
				throw new Error("boom");
			}),
		).rejects.toBeInstanceOf(ArgvPrepareError);
		expect(await prepareOrThrow("x", async () => 7)).toBe(7);
	});

	it("builds an info toast naming the refused action", () => {
		const t = argvFailureToast("reopen the room", { id: "r", name: "room" }, undefined, 1, 0.5);
		expect(t).toMatchObject({ roomId: "r", state: "info" });
		expect(t.message).toBe("Can't reopen the room: couldn't prepare the harness command");
	});

	it("reports through the sink, and only when there is a room", () => {
		const got: NewToast[] = [];
		setArgvFailureSink((t) => got.push(t));
		const quiet = console.error;
		console.error = () => {};
		try {
			reportArgvFailure("create the room", new Error("x"), undefined);
			reportArgvFailure("add the harness", new Error("x"), { id: "r", name: "n" });
		} finally {
			console.error = quiet;
			setArgvFailureSink(null);
		}
		expect(got).toHaveLength(1);
	});
});
