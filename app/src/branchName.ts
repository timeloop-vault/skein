// Pure branch-name logic for the New Room dialog (#227): the worktree
// branch template, slug derivation and `git check-ref-format`-style
// validation. No React, no Tauri — the wiring into App.tsx and
// SettingsModal is a later step.

/**
 * Lowercase, non-alphanumeric-collapsed slug from free-form task text.
 * Moved verbatim from `App.tsx` (#227) — capped at 28 chars, "task" for
 * anything that collapses to nothing.
 */
export const taskSlug = (task: string): string =>
	task
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-|-$/g, "")
		.slice(0, 28) || "task";

/** The template every folder starts with until it remembers its own. */
export const DEFAULT_BRANCH_TEMPLATE = "skein/{slug}";

/**
 * Fill `{slug}` into a branch template. A template with no `{slug}`
 * placeholder gets the slug appended instead — so `feat/` proposes
 * `feat/<slug>` rather than silently dropping the slug on the floor.
 * A blank template is just the slug.
 */
export const applyBranchTemplate = (template: string, slug: string): string => {
	const trimmed = template.trim();
	if (!trimmed) return slug;
	if (trimmed.includes("{slug}")) return trimmed.replaceAll("{slug}", slug);
	return trimmed + slug;
};

/**
 * The template a successful branch name should be remembered as: the
 * prefix up to and including its last `/`, plus `{slug}`. `feat/213-x`
 * → `feat/{slug}`; a branch with no `/` → `{slug}` (matches
 * `DEFAULT_BRANCH_TEMPLATE`'s shape once `skein/` is stripped by the
 * same rule).
 */
export const templateFromBranch = (branch: string): string => {
	const idx = branch.lastIndexOf("/");
	return idx === -1 ? "{slug}" : `${branch.slice(0, idx + 1)}{slug}`;
};

/**
 * Last `/`-segment of a branch, sanitized like `taskSlug` but with no
 * length cap — a user-typed branch name is intentional, not generated
 * from free text. Feeds `git_propose_worktree_path`'s `taskSlug` arg.
 */
export const worktreeLeaf = (branch: string): string => {
	const last = branch.slice(branch.lastIndexOf("/") + 1);
	return (
		last
			.trim()
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, "-")
			.replace(/^-|-$/g, "") || "task"
	);
};

// ── Validation ──────────────────────────────────────────────────────
//
// A rough `git check-ref-format --branch` for a *new* branch name.
// Not exhaustive against every ref grammar edge case (e.g. multiple
// consecutive dots outside "..", or trailing ".lock" nested deeper than
// one component) — just the mistakes a typed name plausibly makes.
// Messages are short: they render inline in a ~82-char slot.

// biome-ignore lint/suspicious/noControlCharactersInRegex: matching git's own ref-format rule
const CONTROL_OR_SPECIAL = /[\x00-\x1f\x7f ~^:?*[\\]/;

export const branchNameProblem = (name: string): string | null => {
	if (!name) return "can't be empty";
	if (name === "@") return 'can\'t be just "@"';
	if (name.startsWith("-")) return "can't start with -";
	if (name.startsWith("/") || name.endsWith("/")) return "can't start or end with /";
	if (name.includes("//")) return "can't contain //";
	if (name.includes("..")) return "can't contain ..";
	if (name.includes("@{")) return "can't contain @{";
	if (name.endsWith(".")) return "can't end with .";
	if (CONTROL_OR_SPECIAL.test(name)) return "contains a space or a special character";
	const parts = name.split("/");
	for (const part of parts) {
		if (!part) return "can't contain //";
		if (part.startsWith(".")) return "no part can start with .";
		if (part.endsWith(".lock")) return "can't end with .lock";
	}
	return null;
};

/** Exact match against the repo's local branch names. */
export const branchCollision = (name: string, existing: readonly string[]): boolean =>
	existing.includes(name);

/**
 * Whether the New Room dialog's branch field should re-attach (go back
 * to following the proposed branch) once it loses focus (#227 review).
 * Detaching itself happens unconditionally on every edit, including an
 * edit to empty — otherwise clear-and-retype would have the field
 * silently refill mid-keystroke before the next character landed.
 * Re-attaching only happens on blur, and only when the field is empty:
 * leaving it blank reads as "never mind, go back to guessing", so it
 * re-attaches and refills from the proposal; blurring on typed content
 * commits to that name.
 */
export const branchFieldAttachedAfterBlur = (value: string): boolean => value.trim() === "";

/**
 * The one problem to show under the New Room dialog's branch field, in
 * priority order: a `git check-ref-format`-style problem with the name
 * itself, then a collision with an existing branch, then a worktree
 * folder already sitting where this branch would create one. `null`
 * when there is nothing to report.
 */
export const branchFieldProblem = (
	name: string,
	existingBranches: readonly string[],
	worktreeFolderExists: boolean,
): string | null => {
	const problem = branchNameProblem(name);
	if (problem) return problem;
	if (branchCollision(name, existingBranches)) return "branch already exists";
	if (worktreeFolderExists) return "worktree folder already exists";
	return null;
};
