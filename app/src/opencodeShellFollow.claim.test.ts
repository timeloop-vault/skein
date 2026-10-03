import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invokeMock(...a) }));

import { startOpencodeShellFollow } from "./opencodeShellFollow.ts";

describe("startOpencodeShellFollow claim lifecycle", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => {
		vi.useRealTimers();
		invokeMock.mockReset();
	});

	it("claims a proven process and does not release on dispose", async () => {
		invokeMock.mockResolvedValue({
			pid: 7,
			sessionId: "ses_a",
			port: 4000,
			portConfirmed: true,
			continueLast: false,
		});
		const claim = { set: vi.fn(), release: vi.fn() };
		const f = startOpencodeShellFollow({
			ptyIdRef: { current: "p" },
			getSessionId: () => undefined,
			getPort: () => undefined,
			onSessionFollowed: undefined,
			claim,
			repoint: vi.fn(),
			hint: vi.fn(),
		});
		f.onOutput();
		await vi.advanceTimersByTimeAsync(1000);
		expect(claim.set).toHaveBeenCalledWith("ses_a", 4000);
		f.dispose();
		await vi.advanceTimersByTimeAsync(60_000);
		expect(claim.release).not.toHaveBeenCalled();
	});

	it("releases when the scan comes back empty", async () => {
		invokeMock.mockResolvedValueOnce({
			pid: 7,
			sessionId: "ses_a",
			port: 4000,
			portConfirmed: true,
			continueLast: false,
		});
		invokeMock.mockResolvedValue(null);
		const claim = { set: vi.fn(), release: vi.fn() };
		const f = startOpencodeShellFollow({
			ptyIdRef: { current: "p" },
			getSessionId: () => undefined,
			getPort: () => undefined,
			onSessionFollowed: undefined,
			claim,
			repoint: vi.fn(),
			hint: vi.fn(),
		});
		f.onOutput();
		await vi.advanceTimersByTimeAsync(1000);
		f.onOutput();
		await vi.advanceTimersByTimeAsync(20_000);
		expect(claim.release).toHaveBeenCalledTimes(1);
		f.dispose();
	});
});
