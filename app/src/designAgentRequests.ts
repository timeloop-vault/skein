// The `design.*` agent requests (#512, #548): panes / state / open_entry /
// set_device / show_element. Called from useAgentRequests; answers via
// `complete`. set_device validates strictly (`validateDevice`, never
// clamping) and persists through the toolbar's own path; it does not need a
// mounted pane. Never changes the active room or harness and never raises
// the window.

import type { RequestResult } from "./agentRequestsShared.ts";
import {
	getDesignPane,
	paneSummaries,
	parseOpenEntryArgs,
	parsePanesArgs,
	parseSetDeviceArgs,
	parseShowElementArgs,
	parseStateArgs,
} from "./designControl.ts";
import { type DesignDevice, normalizeDevice, validateDevice } from "./designDevice.ts";
import { parseInvokeElementArgs } from "./designInvoke.ts";
import { type DesignReveal, revealPane } from "./designReveal.ts";
import type { Room } from "./types.ts";

export type DesignComplete = (id: string, ok?: unknown, error?: string) => Promise<void>;

export const DESIGN_KINDS = [
	"design.panes",
	"design.state",
	"design.open_entry",
	"design.set_device",
	"design.show_element",
	"design.invoke_element",
] as const;

export const isDesignKind = (kind: string): boolean =>
	(DESIGN_KINDS as readonly string[]).includes(kind);

export async function handleDesignRequest(
	kind: string,
	id: string,
	raw: unknown,
	rooms: Room[],
	setEntry: (roomId: string, harnessId: string, entry: string) => void,
	setDevice: (roomId: string, harnessId: string, device: DesignDevice | undefined) => void,
	complete: DesignComplete,
	reveal: DesignReveal,
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

	if (kind === "design.set_device") {
		const p = parseSetDeviceArgs(raw);
		if (bad(p)) return fail(p.error);
		const { roomId, harnessId, device } = p.value;
		const room = rooms.find((r) => r.id === roomId);
		if (!room) return fail(`not_found: room "${roomId}" not found`);
		const harness = room.harnesses.find((h) => h.id === harnessId);
		if (!harness) return fail(`not_found: harness "${harnessId}" not found in room "${roomId}"`);
		if (harness.kind !== "design") {
			return fail(`not_found: harness "${harnessId}" is not a design harness`);
		}
		const v = validateDevice(device);
		if (!v.ok) return fail(`bad_arguments: ${v.error}`);
		const previous = normalizeDevice(harness.designDevice) ?? null;
		setDevice(roomId, harnessId, v.value);
		return complete(id, { device: v.value ?? null, previous });
	}

	if (kind === "design.invoke_element") {
		const p = parseInvokeElementArgs(raw);
		if (bad(p)) return fail(p.error);
		const { roomId, harnessId, reveal: wantReveal, ...req } = p.value;
		const pane = getDesignPane(harnessId);
		if (!pane || pane.roomId !== roomId) {
			return fail(`not_mounted: no design pane is mounted for harness "${harnessId}"`);
		}
		if (!pane.ready()) {
			return fail("not_ready: the design pane has not finished loading its preview yet");
		}
		const revealed = wantReveal ? await revealPane(pane, reveal, roomId, harnessId) : undefined;
		const result = await pane.invokeElement(req);
		return complete(id, revealed === undefined ? result : { ...result, revealed });
	}

	// design.show_element
	const p = parseShowElementArgs(raw);
	if (bad(p)) return fail(p.error);
	const { roomId, harnessId, reveal: wantReveal, ...req } = p.value;
	const pane = getDesignPane(harnessId);
	if (!pane || pane.roomId !== roomId) {
		return fail(`not_mounted: no design pane is mounted for harness "${harnessId}"`);
	}
	if (!pane.ready()) {
		return fail("not_ready: the design pane has not finished loading its preview yet");
	}
	const revealed = wantReveal ? await revealPane(pane, reveal, roomId, harnessId) : undefined;
	const result = await pane.showElement(req);
	return complete(id, revealed === undefined ? result : { ...result, revealed });
}
