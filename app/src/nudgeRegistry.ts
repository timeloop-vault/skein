// The nudge registry (#355 step 1).
//
// #238 shipped three hard-coded review nudges — fixed labels, fixed
// bodies, no way to reword one without a code change. This module turns
// them into data: a fixed list of `NudgeDef`s plus a pure function that
// resolves a def's body against a caller-supplied override map. It adds
// no UI and no storage of its own — `prefs.ts` persists the override
// map, `nudgeStore.ts` makes it reactive for React — this file is just
// the registry and the (still pure, still testable-without-React)
// resolution logic over it.
//
// `scope` is here so an "Actions" nudge (#358's worktree sweep is the
// first) is a one-entry addition to `NUDGES` rather than a second
// parallel list: `nudgesInScope` is the only thing a caller needs to
// add support for a new scope.

import { WORKTREE_SWEEP_BODY } from "./worktreeSweepNudge.ts";

/// Which surface offers a nudge: `review` is #238's existing three,
/// `actions` is #358's, general-purpose ones triggered by the user
/// rather than by review state.
export type NudgeScope = "review" | "actions";

export interface NudgeDef {
	/// Stable identity for an override key. Never derived from the
	/// label — the label is what the user edits.
	id: string;
	label: string;
	/// Shown as help text beside an editable body — what the nudge is
	/// for, not restated in the label.
	description: string;
	defaultBody: string;
	scope: NudgeScope;
}

/// A stored override map: nudge id → body text the user has edited in.
/// An id with no entry means "use `defaultBody`" — the same "absence,
/// not a name" convention `prefs.ts`'s `DefaultAgents` uses for the same
/// reason: there is no override string a user could type that collapses
/// back into "no override".
export type NudgeOverrides = Readonly<Record<string, string>>;

export const NUDGES: readonly NudgeDef[] = [
	{
		id: "review-land",
		label: "Nudge: land",
		description:
			"Sent when the review is signed off — tells the agent it's clear to land the branch.",
		defaultBody: "The review is signed off. Land this branch per this repo's conventions.",
		scope: "review",
	},
	{
		id: "review-lapsed",
		label: "Nudge: lapsed",
		description:
			"Sent when an earlier sign-off has lapsed because the agent committed again — tells it to check.",
		defaultBody:
			"I approved an earlier commit; review has lapsed since you committed. Check review_status.",
		scope: "review",
	},
	{
		id: "review-address-comments",
		label: "Nudge: address comments",
		description: "Sent when there are unresolved review comments and nothing is signed off yet.",
		defaultBody: "Read the open review comments in Skein (list_comments) and address them.",
		scope: "review",
	},
	{
		id: "worktree-sweep",
		label: "Worktree sweep",
		description:
			"Asks the agent to remove worktrees and branches that already landed — shows the plan and waits for your yes first.",
		defaultBody: WORKTREE_SWEEP_BODY,
		scope: "actions",
	},
];

/// `def`'s body: `overrides[def.id]` if present and non-blank, else
/// `def.defaultBody`. The blank check trims to decide, but a passing
/// override is returned verbatim — leading/trailing whitespace a user
/// deliberately typed is theirs to keep.
export function nudgeBody(def: NudgeDef, overrides: NudgeOverrides = {}): string {
	const override = overrides[def.id];
	return override !== undefined && override.trim() !== "" ? override : def.defaultBody;
}

/// Is `def` currently overridden — i.e. would `nudgeBody` return
/// something other than `def.defaultBody`?
export function isOverridden(def: NudgeDef, overrides: NudgeOverrides = {}): boolean {
	return nudgeBody(def, overrides) !== def.defaultBody;
}

/// `overrides` with `id` set to `body` — except a `body` that is blank
/// or textually equal to the def's own default REMOVES the key instead
/// of storing it, so "edit a nudge back to its default text" is the
/// same action as "reset it": there is exactly one representation of
/// "not overridden", not two that happen to resolve the same way. The
/// default is looked up in `NUDGES` by `id`; an `id` that names no known
/// def (a stale one, say) has nothing to compare against, so only the
/// blank check applies.
export function withOverride(overrides: NudgeOverrides, id: string, body: string): NudgeOverrides {
	const defaultBody = NUDGES.find((d) => d.id === id)?.defaultBody;
	if (body.trim() === "" || body === defaultBody) {
		return withoutOverride(overrides, id);
	}
	return { ...overrides, [id]: body };
}

/// `overrides` with `id` reset to "use the default" — an absent key,
/// never a stored default string.
export function withoutOverride(overrides: NudgeOverrides, id: string): NudgeOverrides {
	if (!(id in overrides)) return overrides;
	const next = { ...overrides };
	delete next[id];
	return next;
}

/// Every def in `scope`, in registry order.
export function nudgesInScope(scope: NudgeScope): readonly NudgeDef[] {
	return NUDGES.filter((d) => d.scope === scope);
}

/// Convenience alias for the Actions surface — its own export because
/// callers (the Actions ▾ menu) want a single call site, not a scope
/// string to remember.
export function actionNudges(): readonly NudgeDef[] {
	return nudgesInScope("actions");
}
