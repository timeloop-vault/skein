// The harness tab row's Actions ▾ button (#355 step 2).
//
// #238's Nudge button pastes one of three FIXED, state-selected review
// prompts. This button is the general case: every "actions"-scope nudge
// in the registry (#358 adds the first), always the room's active
// harness, chosen by the user rather than by review state. Reuses the
// same seam — `nudgeRegistry.ts` for the list + body resolution,
// `harnessInput.ts`'s `canSendPrompt` for the per-body safety gate — so
// a nudge that isn't safe to send right now is disabled with the same
// reason a caller would show anywhere else, not a special case.
//
// `actionsButtonState` takes the nudge list and a gate function as
// parameters rather than reaching for the registry/store itself, so it
// stays testable with an empty or fake list without waiting on #358.

import type { GateResult } from "./harnessInput.ts";
import { type NudgeDef, type NudgeOverrides, nudgeBody } from "./nudgeRegistry.ts";

export interface ActionsMenuItem {
	id: string;
	label: string;
	/** The resolved body's first line — what a caller shows as the
	 *  item's tooltip when it's enabled. Multi-line bodies are common
	 *  (#238's own three all are), so the menu never shows a whole
	 *  prompt as a hover tooltip. */
	title: string;
	body: string;
	gate: GateResult;
}

export type ActionsButtonState =
	| { kind: "hidden" }
	| { kind: "disabled"; reason: string }
	| { kind: "menu"; items: readonly ActionsMenuItem[] };

/** `body`'s first line — a single-line body is its own first line. */
export function firstLine(body: string): string {
	const nl = body.indexOf("\n");
	return nl === -1 ? body : body.slice(0, nl);
}

/** The button's state, computed fresh from its inputs — no store reads
 *  here, so a caller (or a test) supplies the harness's nudge list and
 *  a gate function.
 *
 *   - `hidden`: the active harness's kind has no terminal at all
 *     (`files`) — same rule the #238 Nudge button uses, and for the
 *     same reason: there is nowhere to paste.
 *   - `disabled`: the kind has a terminal, but the actions scope is
 *     empty — "no actions yet" until #358 adds the first one.
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
		return { id: def.id, label: def.label, title: firstLine(body), body, gate: gateFor(body) };
	});
	return { kind: "menu", items };
}
