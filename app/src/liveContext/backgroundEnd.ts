// What a `background_end` row says (#447). Pure, so the wording — above
// all that only a real `completed` ever reads as done — is table-tested.

import { num, str } from "./payload.ts";

export interface BackgroundEndView {
	kind: string;
	target: string;
	outcome: string;
	failed: boolean;
	durationMs: number | null;
}

const TARGET_MAX = 80;

function firstLine(text: string): string | null {
	const line = text.split(/\r?\n/).find((l) => l.trim() !== "");
	return line ? line.trim() : null;
}

function truncate(text: string): string {
	return text.length > TARGET_MAX ? `${text.slice(0, TARGET_MAX - 1)}…` : text;
}

function outcomeOf(status: string | null, exit: number | null): { text: string; failed: boolean } {
	switch (status) {
		case "completed":
			return exit == null || exit === 0
				? { text: "done", failed: false }
				: { text: `exit ${exit}`, failed: true };
		case "failed":
			return { text: exit != null ? `failed · exit ${exit}` : "failed", failed: true };
		case "killed":
			return { text: "killed", failed: false };
		case "stopped":
			return { text: "stopped", failed: false };
		case "expired":
			return { text: "timed out", failed: false };
		case "task_stopped":
			return { text: "stopped by agent", failed: false };
		default:
			return { text: "ended", failed: false };
	}
}

export function backgroundEndView(payload: Record<string, unknown>): BackgroundEndView {
	const command = str(payload.command);
	const target =
		str(payload.description) ??
		(command !== undefined ? firstLine(command) : null) ??
		str(payload.task_id) ??
		"task";
	const { text, failed } = outcomeOf(str(payload.status) ?? null, num(payload.exit_code) ?? null);
	return {
		kind: str(payload.task_kind) ?? "task",
		target: truncate(target),
		outcome: text,
		failed,
		durationMs: num(payload.duration_ms) ?? null,
	};
}
