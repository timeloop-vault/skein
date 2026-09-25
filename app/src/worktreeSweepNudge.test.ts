import { describe, expect, it } from "vitest";
import { WORKTREE_SWEEP_BODY } from "./worktreeSweepNudge.ts";

describe("WORKTREE_SWEEP_BODY", () => {
	it("checks Skein rooms via the find_rooms_for_path verb", () => {
		expect(WORKTREE_SWEEP_BODY).toContain("find_rooms_for_path");
		expect(WORKTREE_SWEEP_BODY).toContain("safe_to_remove");
		expect(WORKTREE_SWEEP_BODY).toContain("unreadable_rooms");
	});

	it("checks landed-ness with git cherry and reads worktree state with --porcelain", () => {
		expect(WORKTREE_SWEEP_BODY).toContain("git cherry");
		expect(WORKTREE_SWEEP_BODY).toContain("--porcelain");
	});

	it("requires an explicit yes and refuses to remove anything before it", () => {
		expect(WORKTREE_SWEEP_BODY).toMatch(/explicit yes/);
		expect(WORKTREE_SWEEP_BODY).toMatch(/Do NOT remove anything until/);
	});

	it("never passes --force, except in the sentence that forbids it", () => {
		const re = /--force/g;
		let match: RegExpExecArray | null;
		let count = 0;
		// biome-ignore lint/suspicious/noAssignInExpressions: standard regex-exec-in-while idiom
		while ((match = re.exec(WORKTREE_SWEEP_BODY)) !== null) {
			count++;
			const before = WORKTREE_SWEEP_BODY.slice(Math.max(0, match.index - 20), match.index);
			expect(before).toContain("Never pass");
		}
		expect(count).toBeGreaterThan(0);
	});

	it("is generic — no repo-specific, client-specific or machine-specific strings", () => {
		const forbidden = [
			"skein.db",
			"skein-wt",
			"skein/",
			"bun ",
			"rooms.js",
			"timeloop",
			"stefa",
			"AskUserQuestion",
			"mcp__",
			"C:/",
			"/Users/",
			"APPDATA",
		];
		const lower = WORKTREE_SWEEP_BODY.toLowerCase();
		for (const needle of forbidden) {
			expect(lower).not.toContain(needle.toLowerCase());
		}
	});

	it("is editable and resettable through the nudge override helpers", async () => {
		const { withOverride, nudgeBody, actionNudges } = await import("./nudgeRegistry.ts");
		const def = actionNudges()[0];
		if (!def) throw new Error("expected the worktree-sweep def to be registered");

		const overridden = withOverride({}, "worktree-sweep", "x");
		expect(nudgeBody(def, overridden)).toBe("x");

		const reset = withOverride(overridden, "worktree-sweep", WORKTREE_SWEEP_BODY);
		expect(reset).toEqual({});
	});
});
