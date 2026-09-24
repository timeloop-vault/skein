// Whether to paste a mailbox nudge into a waiting harness (#329), plus the
// nudge text itself. Pure and DOM-free — the caller supplies the #238
// gate's `GateResult` rather than this module re-deriving it, the same
// split `canSendPrompt` itself already draws from its inputs.

import type { ActivityPhase } from "./harnessActivityTypes.ts";
import type { GateResult } from "./harnessInput.ts";

export interface DecideMailNudgeInput {
	phase: ActivityPhase;
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
/// `lastNudged`, `phase === "waiting"` and `gate.ok`. A read lowers
/// `unread` below `lastNudged`, which resets the remembered value down
/// to the new (lower) `unread` — so mail arriving after a full read
/// nudges again, and mail arriving after a partial read only nudges once
/// the count exceeds what is left unread. The reset applies regardless
/// of phase or gate, since it reflects the mailbox being read, not
/// anything about whether a nudge could be sent right now.
///
/// When the phase isn't `waiting` or the gate refuses, nothing is
/// recorded as nudged beyond that reset — the next time the harness
/// reaches `waiting` with mail still outstanding, it nudges.
export function decideMailNudge(input: DecideMailNudgeInput): DecideMailNudgeResult {
	const { phase, unread, gate } = input;
	const lastNudged = unread < input.lastNudged ? unread : input.lastNudged;
	if (phase === "waiting" && gate.ok && unread > lastNudged && unread > 0) {
		return { nudge: true, lastNudged: unread };
	}
	return { nudge: false, lastNudged };
}

/// One-line nudge text, never the message bodies. Singular "message" for
/// one unread; `fromRoomNames` are joined with ", " and the "from …"
/// clause is omitted entirely when the list is empty.
export function mailNudgeText(count: number, fromRoomNames: string[]): string {
	const noun = count === 1 ? "message" : "messages";
	const from = fromRoomNames.length > 0 ? ` from ${fromRoomNames.join(", ")}` : "";
	return `You have ${count} new ${noun} in Skein${from}. Call read_messages.`;
}
