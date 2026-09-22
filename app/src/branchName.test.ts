import { describe, expect, it } from "vitest";
import {
	DEFAULT_BRANCH_TEMPLATE,
	applyBranchTemplate,
	branchCollision,
	branchFieldAttachedAfterBlur,
	branchFieldProblem,
	branchNameProblem,
	taskSlug,
	templateFromBranch,
	worktreeLeaf,
} from "./branchName.ts";

describe("taskSlug", () => {
	it.each([
		["Fix the thing", "fix-the-thing"],
		["  spaced out  ", "spaced-out"],
		["UPPER-CASE_stuff", "upper-case-stuff"],
		["", "task"],
		["   ", "task"],
		["!!!", "task"],
		["a".repeat(40), "a".repeat(28)],
		["日本語 task", "task"],
	])("%s -> %s", (input, expected) => {
		expect(taskSlug(input)).toBe(expected);
	});
});

describe("DEFAULT_BRANCH_TEMPLATE", () => {
	it("is skein/{slug}", () => {
		expect(DEFAULT_BRANCH_TEMPLATE).toBe("skein/{slug}");
	});
});

describe("applyBranchTemplate", () => {
	it("fills {slug} wherever it appears", () => {
		expect(applyBranchTemplate("skein/{slug}", "fix-x")).toBe("skein/fix-x");
		expect(applyBranchTemplate("{slug}/{slug}", "x")).toBe("x/x");
	});

	it("appends the slug when the template has no placeholder", () => {
		expect(applyBranchTemplate("feat/", "fix-x")).toBe("feat/fix-x");
		expect(applyBranchTemplate("feat", "fix-x")).toBe("featfix-x");
	});

	it("is just the slug for a blank or whitespace template", () => {
		expect(applyBranchTemplate("", "fix-x")).toBe("fix-x");
		expect(applyBranchTemplate("   ", "fix-x")).toBe("fix-x");
	});

	it("trims surrounding whitespace on the template", () => {
		expect(applyBranchTemplate("  skein/{slug}  ", "fix-x")).toBe("skein/fix-x");
	});
});

describe("templateFromBranch", () => {
	it("keeps the prefix up to and including the last slash", () => {
		expect(templateFromBranch("feat/213-x")).toBe("feat/{slug}");
		expect(templateFromBranch("skein/fix-thing")).toBe("skein/{slug}");
		expect(templateFromBranch("a/b/c-thing")).toBe("a/b/{slug}");
	});

	it("is just {slug} for a branch with no slash", () => {
		expect(templateFromBranch("foo")).toBe("{slug}");
	});
});

describe("worktreeLeaf", () => {
	it("takes the last slash-segment", () => {
		expect(worktreeLeaf("feat/My Thing")).toBe("my-thing");
		expect(worktreeLeaf("solo-branch")).toBe("solo-branch");
	});

	it("has no length cap, unlike taskSlug", () => {
		const long = "x".repeat(60);
		expect(worktreeLeaf(`feat/${long}`)).toBe(long);
	});

	it("falls back to task for an empty or unsanitizable leaf", () => {
		expect(worktreeLeaf("feat/")).toBe("task");
		expect(worktreeLeaf("feat/!!!")).toBe("task");
	});
});

describe("branchNameProblem", () => {
	const ok = (name: string) => expect(branchNameProblem(name)).toBeNull();
	const bad = (name: string) => expect(branchNameProblem(name)).not.toBeNull();

	it("accepts ordinary names", () => {
		ok("skein/fix-thing");
		ok("feat/213-x");
		ok("main");
	});

	it("rejects empty", () => {
		bad("");
	});

	it("rejects a space", () => {
		bad("feat/my thing");
	});

	it("rejects control characters", () => {
		bad("feat/x\ty");
		bad("feat/x\ny");
	});

	it("rejects the special characters ~ ^ : ? * [ \\", () => {
		for (const ch of ["~", "^", ":", "?", "*", "[", "\\"]) {
			bad(`feat/x${ch}y`);
		}
	});

	it("rejects ..", () => {
		bad("a..b");
		bad("feat/a..b");
	});

	it("rejects @{", () => {
		bad("feat/x@{y");
	});

	it("rejects exactly @", () => {
		bad("@");
	});

	it("accepts @ inside a longer name", () => {
		ok("feat/x@y");
	});

	it("rejects a name starting with -", () => {
		bad("-feat");
	});

	it("rejects a component starting with .", () => {
		bad(".feat");
		bad("feat/.hidden");
	});

	it("rejects a component ending with .lock", () => {
		bad("x.lock");
		bad("feat/x.lock");
	});

	it("rejects empty components (leading/trailing slash, double slash)", () => {
		bad("/feat");
		bad("feat/");
		bad("a//b");
	});

	it("rejects a name ending with .", () => {
		bad("feat.");
	});
});

describe("branchCollision", () => {
	it("is true for an exact match", () => {
		expect(branchCollision("main", ["main", "dev"])).toBe(true);
	});

	it("is false for no match, including a near-miss", () => {
		expect(branchCollision("feat/x", ["main", "dev"])).toBe(false);
		expect(branchCollision("main2", ["main"])).toBe(false);
	});

	it("is false against an empty list", () => {
		expect(branchCollision("main", [])).toBe(false);
	});
});

describe("branchFieldAttachedAfterBlur", () => {
	it("re-attaches on blur when the field is empty", () => {
		expect(branchFieldAttachedAfterBlur("")).toBe(true);
		expect(branchFieldAttachedAfterBlur("   ")).toBe(true);
	});

	it("stays detached on blur when the field has content", () => {
		expect(branchFieldAttachedAfterBlur("feat/x")).toBe(false);
	});
});

describe("branchFieldProblem", () => {
	it("is null for a clean, available name", () => {
		expect(branchFieldProblem("feat/x", ["main"], false)).toBeNull();
	});

	it("reports a name problem before anything else", () => {
		expect(branchFieldProblem("", ["main"], true)).toBe("can't be empty");
	});

	it("reports a collision when the name is otherwise fine", () => {
		expect(branchFieldProblem("main", ["main", "dev"], false)).toBe("branch already exists");
	});

	it("reports an existing worktree folder last", () => {
		expect(branchFieldProblem("feat/x", ["main"], true)).toBe("worktree folder already exists");
	});
});
