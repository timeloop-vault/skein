import { describe, expect, it } from "vitest";
import { HARNESS_KINDS } from "./data.tsx";
import {
	type RepoSkill,
	type RepoSkillsIo,
	loadRepoSkills,
	parseSkillFrontmatter,
	skillInvocationLine,
} from "./repoSkills.ts";

describe("parseSkillFrontmatter", () => {
	it("parses a plain name/description, command from dirName", () => {
		const text = "---\nname: Worktree Sweep\ndescription: Remove stale worktrees.\n---\n\n# Body\n";
		expect(parseSkillFrontmatter(text, "worktree-sweep")).toEqual({
			command: "worktree-sweep",
			name: "Worktree Sweep",
			description: "Remove stale worktrees.",
		});
	});

	it("parses quoted values, single and double", () => {
		const single = "---\nname: 'My Skill'\ndescription: 'quoted body'\n---\n";
		const double = '---\nname: "My Skill"\ndescription: "quoted body"\n---\n';
		expect(parseSkillFrontmatter(single, "my-skill")).toEqual({
			command: "my-skill",
			name: "My Skill",
			description: "quoted body",
		});
		expect(parseSkillFrontmatter(double, "my-skill")).toEqual({
			command: "my-skill",
			name: "My Skill",
			description: "quoted body",
		});
	});

	it("folds a block scalar's continuation lines to one line", () => {
		const folded = [
			"---",
			"name: my-skill",
			"description: >",
			"  First line of a long",
			"  description that wraps",
			"  across several lines.",
			"---",
			"",
		].join("\n");
		expect(parseSkillFrontmatter(folded, "dir")).toEqual({
			command: "dir",
			name: "my-skill",
			description: "First line of a long description that wraps across several lines.",
		});
	});

	it("joins a literal block scalar's continuation lines with spaces too", () => {
		const literal = [
			"---",
			"name: my-skill",
			"description: |-",
			"  line one",
			"  line two",
			"---",
		].join("\n");
		expect(parseSkillFrontmatter(literal, "dir")?.description).toBe("line one line two");
	});

	it("tolerates CRLF line endings", () => {
		const text = "---\r\nname: my-skill\r\ndescription: crlf body\r\n---\r\n";
		expect(parseSkillFrontmatter(text, "dir")).toEqual({
			command: "dir",
			name: "my-skill",
			description: "crlf body",
		});
	});

	it("tolerates a leading UTF-8 BOM", () => {
		const text = "\uFEFF---\nname: my-skill\ndescription: bom body\n---\n";
		expect(parseSkillFrontmatter(text, "dir")).toEqual({
			command: "dir",
			name: "my-skill",
			description: "bom body",
		});
	});

	it("falls back the display name to dirName when frontmatter name is absent", () => {
		const text = "---\ndescription: no name here\n---\n";
		expect(parseSkillFrontmatter(text, "fallback-dir")).toEqual({
			command: "fallback-dir",
			name: "fallback-dir",
			description: "no name here",
		});
	});

	it("defaults description to empty string when absent", () => {
		const text = "---\nname: my-skill\n---\n";
		expect(parseSkillFrontmatter(text, "dir")).toEqual({
			command: "dir",
			name: "my-skill",
			description: "",
		});
	});

	it("sanitizes a frontmatter name that isn't a safe token, since name is display-only", () => {
		const text = "---\nname: Not/A::Safe\tName\n---\n";
		expect(parseSkillFrontmatter(text, "safe-dir")).toEqual({
			command: "safe-dir",
			name: "Not/A::Safe Name",
			description: "",
		});
	});

	it("returns null when dirName (the command) isn't a safe token", () => {
		const text = "---\nname: fine\n---\n";
		expect(parseSkillFrontmatter(text, "not a safe dir")).toBeNull();
		expect(parseSkillFrontmatter(text, "has/a/slash")).toBeNull();
		expect(parseSkillFrontmatter(text, "")).toBeNull();
	});

	it("returns null when dirName exceeds 64 characters", () => {
		const longDir = `a${"b".repeat(64)}`;
		expect(parseSkillFrontmatter("---\nname: fine\n---\n", longDir)).toBeNull();
	});

	it("returns null when there is no frontmatter at all", () => {
		expect(parseSkillFrontmatter("# Just a heading\n\nbody text\n", "dir")).toBeNull();
	});

	it("returns null when the frontmatter is unterminated", () => {
		const text = "---\nname: my-skill\ndescription: never closed\n";
		expect(parseSkillFrontmatter(text, "dir")).toBeNull();
	});

	it("ignores extra top-level keys and nested/indented keys under them", () => {
		const text = [
			"---",
			"name: my-skill",
			"allowed-tools:",
			"  - bash",
			"  - read",
			"description: kept",
			"---",
		].join("\n");
		expect(parseSkillFrontmatter(text, "dir")).toEqual({
			command: "dir",
			name: "my-skill",
			description: "kept",
		});
	});

	it("matches the repo's own worktree-sweep SKILL.md frontmatter shape", () => {
		const text = [
			"---",
			"name: worktree-sweep",
			"description: Remove every git worktree and local branch whose work has already landed on main, in one pass. Use when the user asks to clean up worktrees, prune branches, or tidy the checkout after a run of merged PRs.",
			"---",
			"",
			"# Worktree Sweep",
			"",
		].join("\n");
		const result = parseSkillFrontmatter(text, "worktree-sweep");
		expect(result?.command).toBe("worktree-sweep");
		expect(result?.name).toBe("worktree-sweep");
		expect(result?.description.startsWith("Remove every git worktree")).toBe(true);
	});

	it("matches the repo's own release-workflow SKILL.md frontmatter shape", () => {
		const text = [
			"---",
			"name: release-workflow",
			"description: Draft a Skein GitHub release whose title, opening, section order, depth and voice match the recent releases (v0.3.0, v0.2.15, v0.2.14), and create it as a DRAFT only.",
			"---",
			"",
			"# Release Workflow",
			"",
		].join("\n");
		expect(parseSkillFrontmatter(text, "release-workflow")?.command).toBe("release-workflow");
	});
});

describe("skillInvocationLine", () => {
	it("fills the {name} placeholder in with the command", () => {
		expect(skillInvocationLine("/{name}", "worktree-sweep")).toBe("/worktree-sweep");
		expect(skillInvocationLine("Use the /{name} skill.", "worktree-sweep")).toBe(
			"Use the /worktree-sweep skill.",
		);
	});

	it("every non-null HARNESS_KINDS template contains {name} and is a single line", () => {
		for (const meta of Object.values(HARNESS_KINDS)) {
			if (meta.skillInvocation === null) continue;
			expect(meta.skillInvocation).toContain("{name}");
			expect(meta.skillInvocation).not.toMatch(/[\r\n]/);
			expect(skillInvocationLine(meta.skillInvocation, "worktree-sweep")).toContain(
				"worktree-sweep",
			);
		}
	});
});

describe("loadRepoSkills", () => {
	const skill = (command: string, description = ""): RepoSkill => ({
		command,
		name: command,
		description,
	});

	const fakeIo = (opts: {
		entries?: { name: string; kind: "file" | "dir" | "symlink" }[];
		listDirError?: boolean;
		files?: Record<string, string>;
	}): RepoSkillsIo => ({
		listDir: async () => {
			if (opts.listDirError) throw new Error("ENOENT");
			return opts.entries ?? [];
		},
		readFile: async (path: string) => {
			const content = opts.files?.[path];
			if (content === undefined) throw new Error(`no such file: ${path}`);
			return content;
		},
	});

	const frontmatter = (name: string, description = "") =>
		`---\nname: ${name}\ndescription: ${description}\n---\n`;

	it("returns [] when .claude/skills is missing (listDir rejects)", async () => {
		const io = fakeIo({ listDirError: true });
		expect(await loadRepoSkills("/repo", io)).toEqual([]);
	});

	it("skips a plain file entry and reads dir/symlink entries", async () => {
		const io = fakeIo({
			entries: [
				{ name: "not-a-dir.txt", kind: "file" },
				{ name: "alpha", kind: "dir" },
				{ name: "beta", kind: "symlink" },
			],
			files: {
				"/repo/.claude/skills/alpha/SKILL.md": frontmatter("alpha", "alpha skill"),
				"/repo/.claude/skills/beta/SKILL.md": frontmatter("beta", "beta skill"),
			},
		});
		expect(await loadRepoSkills("/repo", io)).toEqual([
			skill("alpha", "alpha skill"),
			skill("beta", "beta skill"),
		]);
	});

	it("skips an entry whose SKILL.md is unreadable, keeping the others", async () => {
		const io = fakeIo({
			entries: [
				{ name: "good", kind: "dir" },
				{ name: "missing", kind: "dir" },
			],
			files: {
				"/repo/.claude/skills/good/SKILL.md": frontmatter("good"),
			},
		});
		expect(await loadRepoSkills("/repo", io)).toEqual([skill("good")]);
	});

	it("skips an entry whose SKILL.md fails to parse", async () => {
		const io = fakeIo({
			entries: [
				{ name: "good", kind: "dir" },
				{ name: "bad", kind: "dir" },
			],
			files: {
				"/repo/.claude/skills/good/SKILL.md": frontmatter("good"),
				"/repo/.claude/skills/bad/SKILL.md": "no frontmatter here\n",
			},
		});
		expect(await loadRepoSkills("/repo", io)).toEqual([skill("good")]);
	});

	it("sorts by command", async () => {
		const io = fakeIo({
			entries: [
				{ name: "zeta", kind: "dir" },
				{ name: "alpha", kind: "dir" },
			],
			files: {
				"/repo/.claude/skills/zeta/SKILL.md": frontmatter("zeta"),
				"/repo/.claude/skills/alpha/SKILL.md": frontmatter("alpha"),
			},
		});
		expect((await loadRepoSkills("/repo", io)).map((s) => s.command)).toEqual(["alpha", "zeta"]);
	});

	it("deduplicates by command, first after sort wins", async () => {
		// dirName IS the command, so this models a `list_dir` returning the
		// same directory name twice (a symlink aliasing a real dir, say)
		// rather than two distinct directories, which could never collide.
		const collidingIo: RepoSkillsIo = {
			listDir: async () => [
				{ name: "same-name", kind: "dir" },
				{ name: "same-name", kind: "symlink" },
			],
			readFile: async () => frontmatter("same-name", "only one should survive"),
		};
		const result = await loadRepoSkills("/repo", collidingIo);
		expect(result).toHaveLength(1);
		expect(result[0]?.command).toBe("same-name");
	});

	it("joins a Windows-style cwd path with the right separator", async () => {
		const io = fakeIo({
			entries: [{ name: "alpha", kind: "dir" }],
			files: {
				"C:\\repo\\.claude\\skills\\alpha\\SKILL.md": frontmatter("alpha"),
			},
		});
		expect(await loadRepoSkills("C:\\repo", io)).toEqual([skill("alpha")]);
	});
});
