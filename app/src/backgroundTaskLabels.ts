// Pure wording for a harness's live background tasks (#447, epic #439):
// the status-label fragment and the one-line-per-task text the popover
// prints. No store, no React — `BackgroundTaskEntry` is the only input.
// Wording rule: Skein never says a task finished or is done; an overdue
// task is only one the turn is no longer held for.

import { type BackgroundTaskEntry, deadlineOf } from "./backgroundTasks.ts";
import type { BackgroundTaskKind } from "./harnessEvents.ts";

const TITLE_MAX = 60;

const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? "" : "s"}`;

/// The running-status label while work is outstanding: "delegating · 2
/// agents", "background · 3 tasks", or "delegating · 2 agents, 1 task".
/// `null` when nothing is outstanding, so the caller falls back to
/// the bare status.
export function workLabel(agents: number, tasks: number): string | null {
	if (agents > 0 && tasks > 0) {
		return `delegating · ${plural(agents, "agent")}, ${plural(tasks, "task")}`;
	}
	if (agents > 0) return `delegating · ${plural(agents, "agent")}`;
	if (tasks > 0) return `background · ${plural(tasks, "task")}`;
	return null;
}

export function taskKindLabel(kind: BackgroundTaskKind): string {
	switch (kind) {
		case "bash":
			return "bash";
		case "powershell":
			return "powershell";
		case "monitor":
			return "monitor";
	}
}

/// What to call a task: its description, else the first line of its
/// command (truncated), else "task".
export function taskTitle(entry: Pick<BackgroundTaskEntry, "description" | "command">): string {
	const description = entry.description?.trim();
	if (description) return description;
	const firstLine = entry.command?.trim().split(/\r?\n/, 1)[0]?.trim();
	if (!firstLine) return "task";
	return firstLine.length > TITLE_MAX ? `${firstLine.slice(0, TITLE_MAX - 1)}…` : firstLine;
}

/// Compact duration: "45s", "3m 12s", "1h 4m". Negative reads as 0s.
export function formatElapsed(ms: number): string {
	const total = Math.max(0, Math.floor(ms / 1000));
	if (total < 60) return `${total}s`;
	const minutes = Math.floor(total / 60);
	if (minutes < 60) return `${minutes}m ${total % 60}s`;
	return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/// One popover line: `bash "build the app" · 3m 12s`. A Monitor with a
/// deadline adds `· 4m left` (`· due` once past); `(pre-restart)` marks an
/// attach-seeded task, `(no longer waited on)` one the ceiling stopped
/// holding the turn for.
export function taskLine(entry: BackgroundTaskEntry, now: number): string {
	let line = `${taskKindLabel(entry.kind)} "${taskTitle(entry)}" · ${formatElapsed(now - entry.startedAt)}`;
	const deadline = deadlineOf(entry);
	if (deadline !== null) {
		line += deadline <= now ? " · due" : ` · ${formatElapsed(deadline - now)} left`;
	}
	if (entry.fromAttach) line += " (pre-restart)";
	if (entry.overdue) line += " (no longer waited on)";
	return line;
}
