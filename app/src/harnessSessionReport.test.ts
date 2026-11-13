import { beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { reportHarnessSession } from "./harnessSessionReport.ts";
import { forgetReported, pendingSessionIds } from "./reportedSessions.ts";
import { useHarnessWriteBacks } from "./useHarnessWriteBacks.ts";

beforeEach(() => {
	invokeMock.mockReset();
	invokeMock.mockResolvedValue(true);
});

describe("reportHarnessSession", () => {
	it.each([
		[{ mode: "capture", sessionId: "s1" }],
		[{ mode: "replace", sessionId: "s2" }],
		[{ mode: "clear" }],
	] as const)("invokes harness_session_report with %j", async (report) => {
		await expect(reportHarnessSession("r1", "h1", report)).resolves.toBe(true);
		expect(invokeMock).toHaveBeenCalledWith("harness_session_report", {
			roomId: "r1",
			harnessId: "h1",
			report,
		});
	});

	it("passes a false answer through", async () => {
		invokeMock.mockResolvedValue(false);
		await expect(reportHarnessSession("r1", "h1", { mode: "clear" })).resolves.toBe(false);
	});
});

// The hook calls no React hooks, so it can be driven as a plain function.
describe("useHarnessWriteBacks", () => {
	it("capture and replace report and never write rooms", () => {
		const setRooms = vi.fn();
		const wb = useHarnessWriteBacks(setRooms);
		wb.setHarnessSessionId("r1", "h1", "cap");
		wb.replaceHarnessSessionId("r1", "h1", "rep");
		expect(invokeMock).toHaveBeenNthCalledWith(1, "harness_session_report", {
			roomId: "r1",
			harnessId: "h1",
			report: { mode: "capture", sessionId: "cap" },
		});
		expect(invokeMock).toHaveBeenNthCalledWith(2, "harness_session_report", {
			roomId: "r1",
			harnessId: "h1",
			report: { mode: "replace", sessionId: "rep" },
		});
		expect(setRooms).not.toHaveBeenCalled();
	});

	it("setHarnessShellClaim reports replace and sets only shellClaim locally", () => {
		const setRooms = vi.fn();
		const wb = useHarnessWriteBacks(setRooms);
		wb.setHarnessShellClaim("r1", "h1", "sid", 4000);
		expect(invokeMock).toHaveBeenCalledWith("harness_session_report", {
			roomId: "r1",
			harnessId: "h1",
			report: { mode: "replace", sessionId: "sid" },
		});
		expect(setRooms).toHaveBeenCalledTimes(1);
		const fn = setRooms.mock.calls[0]?.[0] as (p: unknown[]) => unknown[];
		const prev = [
			{
				id: "r1",
				name: "r1",
				harnesses: [{ id: "h1", kind: "claude", name: "h1", sessionId: "old" }],
			},
		];
		const next = fn(prev) as { harnesses: Record<string, unknown>[] }[];
		const h = next[0]?.harnesses[0];
		expect(h?.sessionId).toBe("old");
		expect(h?.shellClaim).toBeDefined();
	});

	it("notes the id before the daemon answers; true keeps it, false or a rejection drops it", async () => {
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		const wb = useHarnessWriteBacks(vi.fn());
		const flush = () => new Promise((r) => setTimeout(r, 0));
		try {
			let resolve: (v: boolean) => void = () => {};
			invokeMock.mockReturnValueOnce(new Promise<boolean>((r) => (resolve = r)));
			wb.setHarnessSessionId("r1", "hn1", "cap");
			expect(pendingSessionIds()).toContain("cap");
			resolve(true);
			await flush();
			expect(pendingSessionIds()).toContain("cap");

			invokeMock.mockReturnValueOnce(
				new Promise<boolean>((r) => {
					resolve = r;
				}),
			);
			wb.replaceHarnessSessionId("r1", "hn2", "rep");
			expect(pendingSessionIds()).toContain("rep");
			resolve(false);
			await flush();
			expect(pendingSessionIds()).not.toContain("rep");

			invokeMock.mockRejectedValueOnce(new Error("x"));
			wb.setHarnessSessionId("r1", "hn3", "boom");
			expect(pendingSessionIds()).toContain("boom");
			await flush();
			expect(pendingSessionIds()).not.toContain("boom");
		} finally {
			for (const id of ["hn1", "hn2", "hn3", "hn4"]) forgetReported(id);
			err.mockRestore();
		}
	});

	it("setHarnessShellClaim notes its session id", () => {
		useHarnessWriteBacks(vi.fn()).setHarnessShellClaim("r1", "hn4", "claimed");
		expect(pendingSessionIds()).toContain("claimed");
		forgetReported("hn4");
	});

	it("a failing report is logged, not thrown", async () => {
		const err = vi.spyOn(console, "error").mockImplementation(() => {});
		invokeMock.mockRejectedValue(new Error("x"));
		useHarnessWriteBacks(vi.fn()).setHarnessSessionId("r1", "h1", "s");
		await new Promise((r) => setTimeout(r, 0));
		expect(err).toHaveBeenCalled();
	});
});
