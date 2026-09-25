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

import type { GateResult } from "./harnessInput.ts";
import { type NudgeDef, type NudgeOverrides, nudgeBody } from "./nudgeRegistry.ts";

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
}

export type ActionsButtonState =
	| { kind: "hidden" }
	| { kind: "disabled"; reason: string }
	| { kind: "menu"; items: readonly ActionsMenuItem[] };

/** The button's state, computed fresh from its inputs — no store reads
 *  here, so a caller (or a test) supplies the harness's nudge list and
 *  a gate function.
 *
 *   - `hidden`: the active harness's kind has no terminal at all
 *     (`files`) — same rule the #238 Nudge button uses, and for the
 *     same reason: there is nowhere to paste.
 *   - `disabled`: the kind has a terminal, but the actions scope is
 *     empty — "no actions yet" (not expected today; #358 shipped the
 *     first entry, but a caller can still pass an empty list).
 *   - `menu`: one item per def, each resolved through `nudgeBody`
 *     against `overrides` and gated individually — a multi-line body
 *     can be refused (no bracketed paste) while a single-line sibling
 *     is still sendable.
 */
export function actionsButtonState(
	hasPty: boolean,
	nudges: readonly NudgeDef[],
	overrides: NudgeOverrides,
	gateFor: (body: string) => GateResult,
): ActionsButtonState {
	if (!hasPty) return { kind: "hidden" };
	if (nudges.length === 0) return { kind: "disabled", reason: "no actions yet" };
	const items = nudges.map((def) => {
		const body = nudgeBody(def, overrides);
		return { id: def.id, label: def.label, title: def.description, body, gate: gateFor(body) };
	});
	return { kind: "menu", items };
}
