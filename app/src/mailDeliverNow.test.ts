import { beforeEach, describe, expect, it, type Mock, vi } from "vitest";

vi.mock("./confirmDialog.ts", () => ({ confirmDialog: vi.fn() }));

import type { ComposerDraft } from "./composerDraft.ts";
import { CLEAN_DRAFT } from "./composerDraft.ts";
import { confirmDialog } from "./confirmDialog.ts";
import type { HarnessInputTarget } from "./harnessInput.ts";
import { harnessInput } from "./harnessInput.ts";
import { DELIVER_NOW_REFUSAL, decideRelease, requestDeliverNow } from "./mailDeliverNow.ts";
import { mailHold } from "./mailHold.ts";
import type { ScreenCell, ScreenSnapshot } from "./promptScreen.ts";

const COLS = 20;
const rowOf = (text: string): ScreenCell[] =>
	Array.from({ length: COLS }, (_, x) => ({ ch: [...text][x] ?? " ", dim: false }));
const RULE = "─".repeat(COLS);
const claudeScreen = (input: string, cursorX: number): ScreenSnapshot => ({
	rows: [rowOf(RULE), rowOf(input), rowOf(RULE)],
	cursorX,
	cursorY: 1,
});
const EMPTY = claudeScreen("❯ ", 2);
const TEXT = claudeScreen("❯ hi", 4);

const TYPED: ComposerDraft = { kind: "typed", chars: 2 };
const UNKNOWN: ComposerDraft = { kind: "unknown" };

describe("decideRelease", () => {
	it("refuses a text reading regardless of draft", () => {
		expect(decideRelease(CLEAN_DRAFT, "text")).toBe("refuse");
		expect(decideRelease(UNKNOWN, "text")).toBe("refuse");
	});
	it("refuses a typed draft even when the screen reads empty", () => {
		expect(decideRelease(TYPED, "empty")).toBe("refuse");
		expect(decideRelease(TYPED, null)).toBe("refuse");
	});
	it("delivers on an empty reading with a non-typed draft", () => {
		expect(decideRelease(CLEAN_DRAFT, "empty")).toBe("deliver");
		expect(decideRelease(UNKNOWN, "empty")).toBe("deliver");
	});
	it("confirms on an unknown or missing reading", () => {
		expect(decideRelease(CLEAN_DRAFT, "unknown")).toBe("confirm");
		expect(decideRelease(UNKNOWN, null)).toBe("confirm");
	});
});

let n = 0;
const register = (target: Partial<HarnessInputTarget>) => {
	const id = `dn_${++n}`;
	harnessInput.register(id, {
		paste: vi.fn(),
		bracketedPaste: () => false,
		submit: vi.fn(),
		...target,
	});
	return id;
};

describe("harnessInput.readComposerNow", () => {
	it("reads the screen immediately, with no settle window", () => {
		const id = register({ kind: "claude", screen: () => EMPTY });
		harnessInput.noteDraftEvent(id, { type: "key", key: "a" });
		expect(harnessInput.readComposerNow(id)).toBe("empty");
	});
	it("reads text", () => {
		const id = register({ kind: "claude", screen: () => TEXT });
		expect(harnessInput.readComposerNow(id)).toBe("text");
	});
	it("is null with no target, no screen, no kind, a null snapshot or a throw", () => {
		expect(harnessInput.readComposerNow("nope")).toBeNull();
		expect(harnessInput.readComposerNow(register({ kind: "claude" }))).toBeNull();
		expect(harnessInput.readComposerNow(register({ screen: () => EMPTY }))).toBeNull();
		expect(
			harnessInput.readComposerNow(register({ kind: "claude", screen: () => null })),
		).toBeNull();
		const boom = register({
			kind: "claude",
			screen: () => {
				throw new Error("x");
			},
		});
		expect(harnessInput.readComposerNow(boom)).toBeNull();
	});
});

describe("requestDeliverNow", () => {
	const confirm = vi.mocked(confirmDialog);
	let release: Mock<(harnessId: string) => void>;
	beforeEach(() => {
		confirm.mockReset();
		release = vi.fn();
		mailHold.onRelease(release);
	});

	it("delivers straight away on an empty reading", async () => {
		const id = register({ kind: "claude", screen: () => EMPTY });
		await requestDeliverNow(id);
		expect(release).toHaveBeenCalledWith(id);
		expect(confirm).not.toHaveBeenCalled();
	});

	it("refuses on text: sets the refusal, keeps held, does not release", async () => {
		const id = register({ kind: "claude", screen: () => TEXT });
		mailHold.set(id, true);
		await requestDeliverNow(id);
		expect(release).not.toHaveBeenCalled();
		expect(mailHold.get(id)).toEqual({ held: true, releaseRefusal: DELIVER_NOW_REFUSAL });
		expect(DELIVER_NOW_REFUSAL).toBe("Clear or send your draft first");
	});

	it("confirms on an unreadable screen: yes releases", async () => {
		confirm.mockResolvedValue(true);
		const id = register({});
		await requestDeliverNow(id);
		expect(confirm).toHaveBeenCalledOnce();
		expect(confirm.mock.calls[0]?.[0].message).toBe("The prompt may hold a draft; deliver anyway?");
		expect(release).toHaveBeenCalledWith(id);
	});

	it("confirms on an unreadable screen: no does nothing", async () => {
		confirm.mockResolvedValue(false);
		const id = register({});
		await requestDeliverNow(id);
		expect(release).not.toHaveBeenCalled();
	});
});
