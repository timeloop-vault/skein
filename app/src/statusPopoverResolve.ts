// #132/#329/#331: resolving a hovered chip / dot / mail marker into the
// {kind, status, agent, …} the one-line popover prints. Pure DOM reads
// plus the live harness stores; split out of statusPopover.ts (#460).

import { activityToStatus, harnessActivity } from "./harnessActivity.ts";
import { type TaskState, resolveTasks } from "./statusPopoverTasks.ts";
import { subagents } from "./subagents.ts";

// Rows where a lone status dot describes the same harness as the row's
// chip, so the dot can borrow that chip for its kind (harness tab, feed
// row, status-bar seg). The room tab is excluded: its dot is the room
// *aggregate* and the chip's state comes from the store, not the dot.
// #331: an AGGREGATE dot (room tab, group tab — carries `data-room-ids`,
// see `StatusDot`) never goes through this one-line path at all; it
// gets its own multi-row breakdown popover instead, built fresh from
// `getRooms()` and kept live while shown. A harness-tab/chip dot has no
// `data-room-ids` and is unaffected.
export const ROW_SEL = ".sk-harness-tab, .lc-row, .sk-statusbar .seg";

export interface Resolved {
	kind: string | null;
	status: string | null;
	agent: { key: string; value: string } | null;
	/** #86: the tool a `permission` status is blocked on, when the
	 *  adapter could say. Only ever populated via the chip's live
	 *  harnessId lookup — a lone status dot has no harness id to ask. */
	tool: string | null;
	/** #298: the subagent name when the `permission` dialog belongs to
	 *  one rather than the main session. Same lookup restriction as
	 *  `tool`. */
	agentType: string | null;
	/** #277: how many subagents are currently working, for the
	 *  "delegating · N agents" wording. Only ever populated via the
	 *  chip's live harnessId lookup, same restriction as `tool`. */
	workingCount: number;
	/** #447: live background tasks, same lookup restriction. */
	tasks: TaskState;
	/** #329: this harness's current unread-mail count/senders, read off
	 *  the row's chip regardless of which element (chip, dot, or the ✉
	 *  marker itself) was hovered — so the segment shows up no matter
	 *  where on the tab the pointer is. 0/[] when there's none. */
	mailCount: number;
	mailFrom: readonly string[];
}

// Resolve the {kind, status} to show for a hovered chip/dot. A chip
// contributes the kind; a dot the status. The row supplies the other
// half only when it's unambiguous (exactly one chip / one dot) — so a
// harness tab pairs both, while a room tab's multi-chip row-2 shows
// just the kind and the room dot shows just the state.
export const resolve = (el: HTMLElement): Resolved | null => {
	const isChip = el.classList.contains("h-chip");
	const isDot = el.classList.contains("tab-status");
	const isMail = el.classList.contains("tab-mail");
	if (!isChip && !isDot && !isMail) return null;
	let kind = isChip ? (el.dataset.kind ?? null) : null;
	let status = isDot ? (el.dataset.status ?? null) : null;
	let tool: string | null = null;
	let agentType: string | null = null;
	let workingCount = 0;
	let tasks: TaskState = { workingCount: 0, text: null };
	// #248: the chip carries its harness's agent label, already worded
	// by `agentLabel` — the popover repeats it rather than deciding
	// for itself what an opencode agent can be said to be.
	const chip = isChip
		? el
		: el.closest<HTMLElement>(ROW_SEL)?.querySelector<HTMLElement>(".h-chip[data-agent-key]");
	const agent =
		chip?.dataset.agentKey && chip.dataset.agentValue
			? { key: chip.dataset.agentKey, value: chip.dataset.agentValue }
			: null;
	// A chip knows its harness → read that harness's OWN live state from
	// the store, so a room-tab summary chip shows its real state rather
	// than borrowing the room's aggregate dot (#141). Also picks up
	// `permissionTool` (#86). #329: the mail marker, and a harness-tab
	// dot, borrow the same chip (by id, not by the agent-key-filtered
	// `chip` above, which would miss a Claude harness with no agent), so
	// hovering them shows identical state to the rest of the tab. Feed
	// rows and status-bar segments don't: their dot may not be live state.
	const row = el.closest<HTMLElement>(ROW_SEL);
	const chips = row?.querySelectorAll<HTMLElement>(".h-chip[data-harness-id]");
	const stateChip = isChip
		? el
		: isMail || (isDot && row?.matches(".sk-harness-tab") && chips?.length === 1)
			? chips?.[0]
			: undefined;
	if (stateChip?.dataset.harnessId) {
		const a = harnessActivity.get(stateChip.dataset.harnessId);
		if (a) {
			status = activityToStatus(a);
			tool = a.permissionTool;
			agentType = a.permissionAgentType;
		}
		workingCount = subagents.workingCount(stateChip.dataset.harnessId);
		tasks = resolveTasks(stateChip.dataset.harnessId);
	}
	// A lone status dot (or the mail marker) borrows its row's chip for
	// the kind (harness tab etc.); skipped for the room dot, which is
	// an aggregate.
	if ((isDot || isMail) && !kind) {
		const chips = el.closest<HTMLElement>(ROW_SEL)?.querySelectorAll<HTMLElement>(".h-chip");
		if (chips?.length === 1) kind = chips[0]?.dataset.kind ?? null;
	}
	// #329: unread-mail count/senders, carried on the chip as data
	// attributes (mailStore's own state, but this popover is vanilla
	// DOM with no React access to it) — read for every trigger in the
	// row, so the segment shows whether the chip, dot or ✉ itself was
	// hovered.
	const mailChip = isChip
		? el
		: el.closest<HTMLElement>(ROW_SEL)?.querySelector<HTMLElement>(".h-chip[data-mail-count]");
	const mailCount = mailChip?.dataset.mailCount ? Number(mailChip.dataset.mailCount) : 0;
	let mailFrom: readonly string[] = [];
	if (mailChip?.dataset.mailFrom) {
		try {
			mailFrom = JSON.parse(mailChip.dataset.mailFrom) as string[];
		} catch {
			mailFrom = [];
		}
	}
	return kind || status
		? { kind, status, agent, tool, agentType, workingCount, tasks, mailCount, mailFrom }
		: null;
};
