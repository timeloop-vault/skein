// The outcome→message mapping for #410's "Reattach telemetry" action.
// Pure and separate from `harnessEvents.ts`'s `reattachClaudeTelemetry`
// invoke wrapper so the wording is testable without a Tauri runtime —
// the caller (HarnessActionsMenu's menu item, the command palette
// item) awaits the invoke, then feeds either the resolved
// `ReattachOutcome` or the caught error string in here to get the
// toast text.

import type { ReattachOutcome } from "./harnessEvents.ts";

export type ReattachResult = { ok: true; outcome: ReattachOutcome } | { ok: false; error: string };

export function reattachOutcomeMessage(result: ReattachResult): string {
	if (!result.ok) return `Reattach failed: ${result.error}`;
	switch (result.outcome) {
		case "reattached":
			return "Telemetry reattached; status will update from the transcript.";
		case "healthy":
			return "Telemetry is healthy; nothing to reattach.";
		case "not_attached":
			return "No telemetry tail for this harness; restart the harness to reattach.";
	}
}
