// #331: the aggregate (room tab / group tab dot) breakdown popover —
// building its payload from the live rooms and rendering it. Split out of
// statusPopover.ts (#460).

import { backgroundTasks } from "./backgroundTasks.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity, statusLabel } from "./harnessActivity.ts";
import {
	type Breakdown,
	type BreakdownRoomInput,
	type BreakdownRow,
	type BreakdownSubagent,
	buildBreakdown,
} from "./statusBreakdown.ts";
import { renderTaskLines } from "./statusPopoverTasks.ts";
import { subagents } from "./subagents.ts";
import type { Room } from "./types.ts";

const rule = (): HTMLDivElement => {
	const r = document.createElement("div");
	r.className = "bd-rule";
	return r;
};

const renderRow = (row: BreakdownRow, single: boolean): HTMLDivElement => {
	const div = document.createElement("div");
	div.className = "bd-row";
	const dot = document.createElement("span");
	dot.className = `bd-dot pv-${row.status}`;
	dot.textContent = "●";
	const path = document.createElement("span");
	path.className = "bd-path";
	path.textContent = single ? row.harnessName : `${row.roomName} › ${row.harnessName}`;
	const chip = document.createElement("span");
	const kindMeta = HARNESS_KINDS[row.kind];
	chip.className = `bd-chip ${kindMeta.chip}`;
	chip.textContent = kindMeta.label;
	const label = document.createElement("span");
	label.className = `bd-label pv-${row.status}`;
	label.textContent = row.label;
	div.append(dot, path, chip, label);
	return div;
};

const renderSubagentLine = (sa: BreakdownSubagent): HTMLDivElement => {
	const div = document.createElement("div");
	div.className = sa.preRestart ? "bd-sub bd-sub-pre" : "bd-sub";
	const type = sa.agentType ?? "agent";
	const desc = sa.description ? ` "${sa.description}"` : "";
	const suffix = sa.preRestart ? " (pre-restart)" : "";
	div.textContent = `↳ ${type}${desc}${suffix}`;
	return div;
};

const quietFooterText = (b: Breakdown, single: boolean): string => {
	const parts = b.quiet.map((q) => {
		const kinds = q.kinds.map((k) => HARNESS_KINDS[k].label).join(", ");
		return single ? kinds : `${q.roomName}: ${kinds}`;
	});
	return `+ ${b.quietCount} idle  (${parts.join(" · ")})`;
};

export const renderBreakdown = (
	el: HTMLDivElement,
	aggName: string,
	b: Breakdown,
	single: boolean,
) => {
	el.classList.add("sk-pop-breakdown");
	el.replaceChildren();

	const header = document.createElement("div");
	header.className = "bd-header";
	const nameSpan = document.createElement("span");
	nameSpan.className = "bd-name";
	nameSpan.textContent = aggName;
	const sep = document.createElement("span");
	sep.className = "sep";
	sep.textContent = "·";
	const statusSpan = document.createElement("span");
	statusSpan.className = `pv-${b.status}`;
	statusSpan.textContent = statusLabel(b.status);
	header.append(nameSpan, sep, statusSpan);
	el.appendChild(header);

	if (b.rows.length > 0) {
		el.appendChild(rule());
		for (const row of b.rows) {
			el.appendChild(renderRow(row, single));
			for (const sa of row.subagents) el.appendChild(renderSubagentLine(sa));
			if (row.hiddenSubagents > 0) {
				const more = document.createElement("div");
				more.className = "bd-sub bd-sub-more";
				more.textContent = `↳ + ${row.hiddenSubagents} more`;
				el.appendChild(more);
			}
			el.append(...renderTaskLines(row));
		}
		if (b.moreRows > 0) {
			const more = document.createElement("div");
			more.className = "bd-more";
			more.textContent = `+ ${b.moreRows} more`;
			el.appendChild(more);
		}
	}

	if (b.quietCount > 0) {
		el.appendChild(rule());
		const footer = document.createElement("div");
		footer.className = "bd-footer";
		footer.textContent = quietFooterText(b, single);
		el.appendChild(footer);
	}
};

// Resolve this dot's room ids against the latest `getRooms()`
// snapshot and build the breakdown payload. Re-run on every show
// AND on every live-store emit while shown (see `startBreakdown`) —
// `getRooms()` itself is a live ref read, so a room closed/renamed
// mid-hover is picked up too.
export const buildFor = (
	getRooms: () => readonly Room[],
	roomIds: readonly string[],
): { breakdown: Breakdown; rooms: readonly BreakdownRoomInput[] } => {
	const byId = new Map(getRooms().map((r) => [r.id, r]));
	const rooms: BreakdownRoomInput[] = [];
	for (const id of roomIds) {
		const r = byId.get(id);
		if (!r) continue;
		rooms.push({
			id: r.id,
			name: r.name,
			harnesses: r.harnesses.map((h) => ({
				id: h.id,
				kind: h.kind,
				name: h.name,
				pendingNotifications: h.pendingNotifications,
			})),
		});
	}
	const breakdown = buildBreakdown(rooms, {
		activity: harnessActivity.get,
		subagents: subagents.live,
		workingCount: subagents.workingCount,
		backgroundTasks: backgroundTasks.live,
		workingTaskCount: backgroundTasks.workingCount,
	});
	return { breakdown, rooms };
};
