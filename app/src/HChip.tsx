import { HARNESS_KINDS } from "./data.tsx";
import type { AgentLabel } from "./harnessAgent.ts";
import type { HarnessKind, Status } from "./types.ts";
import "./components.css";

// #68: size is owned by CSS (density --chip / --dot tokens + context
// overrides in components.css), not per-call-site numbers.
// #132: data-kind / data-status feed the shared hover popover
// (statusPopover.ts), which replaces the native title= (slow, unstyled,
// and it couldn't show state). aria-label keeps the info available to
// screen readers.
export const HChip = ({
	kind,
	harnessId,
	agent,
	mailCount,
	mailFromRoomNames,
}: {
	kind: HarnessKind;
	harnessId?: string;
	/** #248: the harness's agent label, for the popover. Omitted where the
	 *  chip is a kind rather than one harness (pickers, room-tab row). */
	agent?: AgentLabel | null | undefined;
	/** #329: this harness's unread-mail count/senders, for the hover
	 *  popover's mail segment. The chip is the one element every trigger
	 *  in the row (chip, dot, the ✉ marker itself) can find, so it's the
	 *  carrier — `statusPopover.ts` has no React access to `mailStore`.
	 *  Omitted (no attribute) when there's no mail to show. */
	mailCount?: number;
	mailFromRoomNames?: readonly string[];
}) => {
	const k = HARNESS_KINDS[kind];
	// #141: harnessId lets the popover read this harness's OWN live state
	// (so a room-tab summary chip shows its real state, not the room
	// aggregate). Omitted where there's no single harness behind the chip.
	return (
		<span
			className={`h-chip ${k.chip}`}
			data-kind={kind}
			data-harness-id={harnessId}
			data-agent-key={agent?.key}
			data-agent-value={agent?.value}
			data-mail-count={mailCount && mailCount > 0 ? mailCount : undefined}
			data-mail-from={
				mailCount && mailCount > 0 ? JSON.stringify(mailFromRoomNames ?? []) : undefined
			}
			role="group"
			aria-label={k.name}
		>
			{k.label}
		</span>
	);
};

export const StatusDot = ({
	status,
	roomIds,
	aggName,
}: {
	status: Status;
	/** #331: room ids this dot aggregates over, so the hover popover can
	 *  build a per-harness breakdown instead of the plain one-line
	 *  summary. A room tab passes its own single id; a group tab passes
	 *  every member room's id. Omitted for a harness-tab/chip dot, which
	 *  keeps today's one-line popover (statusPopover.ts branches on
	 *  whether this is present). */
	roomIds?: readonly string[];
	/** #331: the name to head the breakdown popover with — the room's
	 *  own name, or the group's display name. Meaningless without
	 *  `roomIds`. */
	aggName?: string;
}) => (
	<span
		className={`tab-status st-${status}`}
		data-status={status}
		data-room-ids={roomIds?.join(" ")}
		data-agg-name={aggName}
		role="img"
		aria-label={status}
	/>
);
