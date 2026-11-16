// The mail segment's wording for the harness-tab hover popover (#329).
// The nudge itself — when to send it, its text, the retries — is the
// daemon's since #733; this module keeps only what the client displays.

/// The mail segment's VALUE for the harness-tab hover popover (#329) —
/// `statusPopover.ts` prints it as `mail <this>`. The from-clause is
/// omitted entirely when `fromRoomNames` is empty.
export function mailPopoverText(count: number, fromRoomNames: readonly string[]): string {
	const noun = count === 1 ? "message" : "messages";
	const from = fromRoomNames.length > 0 ? ` from ${fromRoomNames.join(", ")}` : "";
	return `${count} unread ${noun}${from}`;
}
