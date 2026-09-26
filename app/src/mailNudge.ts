// Whether to paste a mailbox nudge into a waiting harness (#329), plus the
// nudge text itself. Pure and DOM-free — the caller supplies the #238
// gate's `GateResult` rather than this module re-deriving it, the same
// split `canSendPrompt` itself already draws from its inputs.

import type { ComposerDraft } from "./composerDraft.ts";
import { checkDraft } from "./composerDraft.ts";
import type { ActivityPhase } from "./harnessActivityTypes.ts";
import type { GateResult } from "./harnessInput.ts";

export interface DecideMailNudgeInput {
	/// #381: whether the harness is at a safe stopping point right now —
	/// `atSafeStoppingPoint`'s own predicate (`waiting`, or `running`
	/// with a #277 delegation deferral armed). A caller has this already
	/// wherever it has an activity snapshot; passing the bool rather than
	/// the whole `HarnessActivity` keeps this module DOM/store-free.
	atStoppingPoint: boolean;
	/// Current unread count for this harness's mailbox.
	unread: number;
	/// The unread count as of the last nudge actually sent (or 0, e.g.
	/// after a restart with no memory of a prior nudge).
	lastNudged: number;
	/// The #238 send gate's own verdict for this harness right now.
	gate: GateResult;
}

export interface DecideMailNudgeResult {
	nudge: boolean;
	/// The next value to remember as `lastNudged`.
	lastNudged: number;
}

/// One nudge per arrival: nudges only when `unread` has grown past
/// `lastNudged`, the harness is at a safe stopping point
/// (`atStoppingPoint`, #381 — `waiting`, or `running` with a #277
/// delegation deferral armed) and `gate.ok`. A read lowers `unread`
/// below `lastNudged`, which resets the remembered value down to the
/// new (lower) `unread` — so mail arriving after a full read nudges
/// again, and mail arriving after a partial read only nudges once the
/// count exceeds what is left unread. The reset applies regardless of
/// stopping point or gate, since it reflects the mailbox being read,
/// not anything about whether a nudge could be sent right now.
///
/// When the harness isn't at a safe stopping point or the gate refuses,
/// nothing is recorded as nudged beyond that reset — the next time it
/// reaches one with mail still outstanding, it nudges.
/// #383: the extra gate an AUTOMATIC nudge folds on top of `canSendPrompt`
/// — a held draft refuses even a `gate.ok` result, so `decideMailNudge`
/// sees the same refusal shape it already knows how to hold on. A `gate`
/// that's already refused is returned unchanged: the underlying reason
/// (not ready, not watched, …) is more specific than a draft guess, and
/// there's no reason to prefer the latter.
export function automaticGate(gate: GateResult, draft: ComposerDraft): GateResult {
	if (!gate.ok) return gate;
	return checkDraft(draft) ?? gate;
}

export function decideMailNudge(input: DecideMailNudgeInput): DecideMailNudgeResult {
	const { atStoppingPoint, unread, gate } = input;
	const lastNudged = unread < input.lastNudged ? unread : input.lastNudged;
	if (atStoppingPoint && gate.ok && unread > lastNudged && unread > 0) {
		return { nudge: true, lastNudged: unread };
	}
	return { nudge: false, lastNudged };
}

/// #381: the transitions listener's trigger predicate — whether a
/// transition landing on `to`, with the harness's freshly-read
/// `atStoppingPointNow` (`atSafeStoppingPoint(harnessActivity.get(id))`
/// at the caller), is worth running a mail `check` for. `to === "waiting"`
/// is the original #329 trigger, kept unconditional since it's always a
/// safe stopping point; `atStoppingPointNow` is what additionally covers
/// `permission → running` while a #277 delegation deferral is still
/// armed — the permission dialog closing can itself be the event that
/// re-enters the deferred state, with no transition into `waiting` at
/// all.
export function shouldCheckOnTransition(to: ActivityPhase, atStoppingPointNow: boolean): boolean {
	return to === "waiting" || atStoppingPointNow;
}

/// One-line nudge text, never the message bodies. Singular "message" for
/// one unread; `fromRoomNames` are joined with ", " and the "from …"
/// clause is omitted entirely when the list is empty.
export function mailNudgeText(count: number, fromRoomNames: string[]): string {
	const noun = count === 1 ? "message" : "messages";
	const from = fromRoomNames.length > 0 ? ` from ${fromRoomNames.join(", ")}` : "";
	return `You have ${count} new ${noun} in Skein${from}. Call read_messages.`;
}

/// The mail segment's VALUE for the harness-tab hover popover (#329) —
/// `statusPopover.ts` prints it as `mail <this>`. Same singular/plural and
/// from-clause rules as `mailNudgeText`, just without the sentence/call
/// wrapping a one-line popover segment doesn't want.
export function mailPopoverText(count: number, fromRoomNames: readonly string[]): string {
	const noun = count === 1 ? "message" : "messages";
	const from = fromRoomNames.length > 0 ? ` from ${fromRoomNames.join(", ")}` : "";
	return `${count} unread ${noun}${from}`;
}
