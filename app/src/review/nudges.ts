// Nudge bodies + selection (#238, registry-backed since #355).
//
// Three prompts, pasted into the room's active harness and submitted.
// Which one applies is a pure function of the review's own state
// (sign-off + unresolved-thread count) — testable without a harness, a
// PTY, or React. The bodies themselves now live as data in
// `nudgeRegistry.ts`, with an optional per-nudge override the user can
// set in Settings (persisted in `prefs.ts`, reactive via
// `nudgeStore.ts`) — this module resolves a def against the caller's
// overrides rather than hard-coding a string.

import { NUDGES, type NudgeDef, type NudgeOverrides, nudgeBody } from "../nudgeRegistry.ts";
import type { SignoffState } from "./signoff.ts";

/// Look up one of the three fixed review nudges by id, throwing at
/// module init (never at call time) if the registry is ever edited out
/// from under this module — a bug loud enough to fail immediately
/// rather than a `selectNudge` that silently returns `undefined`.
function requireDef(id: string): NudgeDef {
	const found = NUDGES.find((d) => d.id === id);
	if (!found) throw new Error(`nudgeRegistry.NUDGES is missing expected nudge "${id}"`);
	return found;
}

const landDef = requireDef("review-land");
const lapsedDef = requireDef("review-lapsed");
const addressCommentsDef = requireDef("review-address-comments");

/// The three review nudges' default bodies — still exported by name for
/// anything (tests included) that wants the shipped-in-code text rather
/// than a possibly-overridden one.
export const NUDGE_LAND = landDef.defaultBody;
export const NUDGE_LAPSED = lapsedDef.defaultBody;
export const NUDGE_ADDRESS_COMMENTS = addressCommentsDef.defaultBody;

export interface Nudge {
	label: string;
	body: string;
}

/// Which nudge (if any) applies right now. `undefined` means there is
/// nothing to nudge about — the caller renders a disabled button rather
/// than omitting it, so "no nudge available" reads as a state rather
/// than as a missing feature. `overrides` defaults to none, so every
/// existing call site keeps returning the shipped-in-code bodies.
export function selectNudge(
	signoff: SignoffState,
	unresolvedCount: number,
	overrides: NudgeOverrides = {},
): Nudge | undefined {
	if (signoff === "approved") {
		return { label: landDef.label, body: nudgeBody(landDef, overrides) };
	}
	if (signoff === "stale") {
		return { label: lapsedDef.label, body: nudgeBody(lapsedDef, overrides) };
	}
	if (unresolvedCount > 0) {
		return { label: addressCommentsDef.label, body: nudgeBody(addressCommentsDef, overrides) };
	}
	return undefined;
}
