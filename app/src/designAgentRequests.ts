// The `design.*` agent requests (#512): panes / state / open_entry /
// show_element. Called from useAgentRequests; answers via `complete`.
// Never changes the active room or harness and never raises the window.

import type { RequestResult } from "./agentRequestsShared.ts";
import {
	getDesignPane,
	paneSummaries,
	parseOpenEntryArgs,
	parsePanesArgs,
	parseShowElementArgs,
	parseStateArgs,
} from "./designControl.ts";
import type { Room } from "./types.ts";

export type DesignComplete = (id: string, ok?: unknown, error?: string) => Promise<void>;

export const DESIGN_KINDS = [
	"design.panes",
	"design.state",
	"design.open_entry",
	"design.show_element",
] as const;

export const isDesignKind = (kind: string): boolean =>
	(DESIGN_KINDS as readonly string[]).includes(kind);

export async function handleDesignRequest(
	kind: string,
	id: string,
	raw: unknown,
	rooms: Room[],
	setEntry: (roomId: string, harnessId: string, entry: string) => void,
	complete: DesignComplete,
): Promise<void> {
	const fail = (error: string) => complete(id, undefined, error);
	const bad = <T>(r: RequestResult<T>): r is { ok: false; error: string } => !r.ok;

	if (kind === "design.panes") {
		const p = parsePanesArgs(raw);
		if (bad(p)) return fail(p.error);
		return complete(id, { panes: paneSummaries(p.value.roomId) });
	}

	if (kind === "design.state") {
		const p = parseStateArgs(raw);
		if (bad(p)) return fail(p.error);
		const pane = getDesignPane(p.value.harnessId);
		if (!pane || pane.roomId !== p.value.roomId) {
			return fail(`not_mounted: no design pane is mounted for harness "${p.value.harnessId}"`);
		}
		return complete(id, pane.getState());
	}

	if (kind === "design.open_entry") {
		const p = parseOpenEntryArgs(raw);
		if (bad(p)) return fail(p.error);
		const { roomId, harnessId, entry } = p.value;
		const room = rooms.find((r) => r.id === roomId);
		if (!room) return fail(`not_found: room "${roomId}" not found`);
		const harness = room.harnesses.find((h) => h.id === harnessId);
		if (!harness) return fail(`not_found: harness "${harnessId}" not found in room "${roomId}"`);
		if (harness.kind !== "design") {
			return fail(`not_found: harness "${harnessId}" is not a design harness`);
		}
		const previous = harness.designEntry ?? null;
		setEntry(roomId, harnessId, entry);
		return complete(id, { entry, previous });
	}

	// design.show_element
	const p = parseShowElementArgs(raw);
	if (bad(p)) return fail(p.error);
	const { roomId, harnessId, ...req } = p.value;
	const pane = getDesignPane(harnessId);
	if (!pane || pane.roomId !== roomId) {
		return fail(`not_mounted: no design pane is mounted for harness "${harnessId}"`);
	}
	if (!pane.ready()) {
		return fail("not_ready: the design pane has not finished loading its preview yet");
	}
	return complete(id, await pane.showElement(req));
}
