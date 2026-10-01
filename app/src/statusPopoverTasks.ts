// Background-task lines for the status popovers (#447). The string
// builders are pure so they can be table-tested; the DOM builders are
// thin wrappers kept here so statusPopover.ts barely grows.

import { taskLine } from "./backgroundTaskLabels.ts";
import { type BackgroundTaskEntry, backgroundTasks } from "./backgroundTasks.ts";
import type { BreakdownRow } from "./statusBreakdown.ts";

const SINGLE_LINES = 2;

/// What the single-harness popover resolves for a chip's harness.
export interface TaskState {
	workingCount: number;
	text: string | null;
}

/// "bash "a" · 5s; monitor "b" · 1m 0s +1 more" — up to two task lines,
/// then a count of the rest. `null` when there are no tasks.
export function tasksSegmentText(
	tasks: readonly BackgroundTaskEntry[],
	now: number,
): string | null {
	if (tasks.length === 0) return null;
	const shown = tasks.slice(0, SINGLE_LINES).map((t) => taskLine(t, now));
	const rest = tasks.length - shown.length;
	return rest > 0 ? `${shown.join("; ")} +${rest} more` : shown.join("; ");
}

export function moreTasksText(hidden: number): string {
	return `↳ + ${hidden} more`;
}

export function resolveTasks(harnessId: string, now = Date.now()): TaskState {
	return {
		workingCount: backgroundTasks.workingCount(harnessId),
		text: tasksSegmentText(backgroundTasks.live(harnessId), now),
	};
}

/// The `↳ …` divs for a breakdown row's tasks, then "+N more" for hidden.
export function renderTaskLines(row: BreakdownRow, now = Date.now()): HTMLDivElement[] {
	const divs = row.tasks.map((task) => {
		const div = document.createElement("div");
		div.className = task.fromAttach || task.overdue ? "bd-sub bd-sub-pre" : "bd-sub";
		div.textContent = `↳ ${taskLine(task, now)}`;
		return div;
	});
	if (row.hiddenTasks > 0) {
		const more = document.createElement("div");
		more.className = "bd-sub bd-sub-more";
		more.textContent = moreTasksText(row.hiddenTasks);
		divs.push(more);
	}
	return divs;
}
