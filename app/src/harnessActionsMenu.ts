// The harness tab row's Actions ▾ button (#355 step 2).
//
// #238's Nudge button pastes one of three FIXED, state-selected review
// prompts. This button is the general case: every "actions"-scope nudge
// in the registry (#358's worktree sweep is the first), always the
// room's active harness, chosen by the user rather than by review
// state. Reuses the same seam — `nudgeRegistry.ts` for the list + body
// resolution, `harnessInput.ts`'s `canSendPrompt` for the per-body
// safety gate — so a nudge that isn't safe to send right now is
// disabled with the same reason a caller would show anywhere else, not
// a special case.
//
// `actionsButtonState` takes the nudge list and a gate function as
// parameters rather than reaching for the registry/store itself, so it
// stays testable with an empty or fake list.
//
// #359 adds a second source of items: the room's own repo skills
// (`repoSkills.ts`), one per `.claude/skills/*/SKILL.md`, each typed in
// as `skillInvocationLine(template, skill.command)` — the exact text
// #238's seam pastes. `skillInvocation === null` (a kind with no slash
// convention) means no skill items at all, regardless of what skills
// were found. `source` on each item is what lets the component draw a
// "Repo skills" divider before the first skill item without the pure
// state needing to know anything about rendering.

import type { GateResult } from "./harnessInput.ts";
import { type NudgeDef, type NudgeOverrides, nudgeBody } from "./nudgeRegistry.ts";
import type { RepoSkill } from "./repoSkills.ts";
import { skillInvocationLine } from "./repoSkills.ts";

export interface ActionsMenuItem {
	id: string;
	label: string;
	/** The def's `description` — what a caller shows as the item's
	 *  tooltip when it's enabled. Bodies run to a page (#358's worktree
	 *  sweep is one), so the menu never shows a whole prompt, or even
	 *  its first line, as a hover tooltip; `description` is written to
	 *  stand alone as one short sentence. */
	title: string;
	body: string;
	gate: GateResult;
	/** `"nudge"` for a registry entry, `"skill"` for a repo skill (#359)
	 *  — lets the component insert a "Repo skills" divider before the
	 *  first skill item without re-deriving the split itself. */
	source: "nudge" | "skill";
}

export type ActionsButtonState =
	| { kind: "hidden" }
	| { kind: "disabled"; reason: string }
	| { kind: "menu"; items: readonly ActionsMenuItem[] };

/** The button's state, computed fresh from its inputs — no store reads
 *  here, so a caller (or a test) supplies the harness's nudge list, its
 *  loaded repo skills and a gate function.
 *
 *   - `hidden`: the active harness's kind has no terminal at all
 *     (`files`) — same rule the #238 Nudge button uses, and for the
 *     same reason: there is nowhere to paste.
 *   - `disabled`: the kind has a terminal, but there is nothing to show
 *     — both the actions scope AND the skill list are empty — "no
 *     actions yet".
 *   - `menu`: one item per nudge def (resolved through `nudgeBody`
 *     against `overrides`), followed by one item per skill (resolved
 *     through `skillInvocationLine` against `skillInvocation` — skipped
 *     entirely when `skillInvocation` is `null`, i.e. the kind has no
 *     slash convention), each gated individually — a multi-line body
 *     can be refused (no bracketed paste) while a single-line sibling
 *     is still sendable.
 */
export function actionsButtonState(
	hasPty: boolean,
	nudges: readonly NudgeDef[],
	overrides: NudgeOverrides,
	gateFor: (body: string) => GateResult,
	skills: readonly RepoSkill[],
	skillInvocation: string | null,
): ActionsButtonState {
	if (!hasPty) return { kind: "hidden" };
	const nudgeItems: ActionsMenuItem[] = nudges.map((def) => {
		const body = nudgeBody(def, overrides);
		return {
			id: def.id,
			label: def.label,
			title: def.description,
			body,
			gate: gateFor(body),
			source: "nudge",
		};
	});
	const skillItems: ActionsMenuItem[] =
		skillInvocation === null
			? []
			: skills.map((skill) => {
					const body = skillInvocationLine(skillInvocation, skill.command);
					return {
						id: `skill:${skill.command}`,
						label: `/${skill.command}`,
						title: skill.description || skill.name,
						body,
						gate: gateFor(body),
						source: "skill",
					};
				});
	if (nudgeItems.length === 0 && skillItems.length === 0) {
		return { kind: "disabled", reason: "no actions yet" };
	}
	return { kind: "menu", items: [...nudgeItems, ...skillItems] };
}
