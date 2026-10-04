// designControl — the agent-facing seam over mounted design panes (#512).
//
// A DesignBody registers a `DesignPaneApi` while it is mounted (same shape
// as `harnessInput`'s per-harness registry: register returns a disposer).
// `designAgentRequests.ts` answers the four `design.*` agent requests from
// it. Nothing here touches focus: it only reads pane state and asks the
// pane to highlight an element in place.

import { asRecord, isOmitted, type RequestResult } from "./agentRequestsShared.ts";
import type { ShowChangesRequest, ShowChangesResult } from "./designChanges.ts";
import type { DesignDevice } from "./designDevice.ts";
import type { InvokeElementRequest, InvokeElementResult } from "./designInvoke.ts";
import type { ElementAnchor, ElementDescriptor, Placement } from "./elementAnchor.ts";

export type DesignScroll = { x: number; y: number };
export type DesignSelection = { threadId: string; anchor: ElementAnchor };

/** Everything `design.state` reports about one pane. */
export interface DesignPaneState {
	entry: string | null;
	device: DesignDevice | null;
	ready: boolean;
	loadFailed: boolean;
	errors: unknown[];
	selected: DesignSelection | null;
	scroll: DesignScroll | null;
}

export type ShowElementRequest = { selector?: string; anchor?: ElementAnchor; reveal?: true };

export type ShowTier = "anchored" | "reanchored" | "selector" | "stale" | "ambiguous" | "not_found";

export interface ShowElementResult {
	tier: ShowTier;
	highlighted: boolean;
	element: ElementDescriptor | null;
	count?: number;
	score?: number;
	/** The selector was not valid CSS. */
	invalidSelector?: true;
}

/** What a mounted DesignBody must implement. */
export interface DesignPaneApi {
	roomId: string;
	getState(): DesignPaneState;
	/** True once the iframe has fired its ready beacon. */
	ready(): boolean;
	/** Resolves true once the pane is on screen and laid out, false on timeout (#549). */
	whenVisible(timeoutMs: number): Promise<boolean>;
	showElement(req: ShowElementRequest): Promise<ShowElementResult>;
	invokeElement(req: InvokeElementRequest): Promise<InvokeElementResult>;
	showChanges(req: ShowChangesRequest): Promise<ShowChangesResult>;
}

const panes = new Map<string, DesignPaneApi>();

/** Register a mounted pane; call the returned disposer on unmount. */
export function registerDesignPane(harnessId: string, api: DesignPaneApi): () => void {
	panes.set(harnessId, api);
	return () => {
		if (panes.get(harnessId) === api) panes.delete(harnessId);
	};
}

export function getDesignPane(harnessId: string): DesignPaneApi | undefined {
	return panes.get(harnessId);
}

export type PaneSummary = { harnessId: string; mounted: boolean; ready: boolean };

/** Every registered pane of a room. */
export function paneSummaries(roomId: string): PaneSummary[] {
	const out: PaneSummary[] = [];
	for (const [harnessId, api] of panes) {
		if (api.roomId !== roomId) continue;
		out.push({ harnessId, mounted: true, ready: api.ready() });
	}
	return out;
}

/** `matchElement`'s verdict as the verb's result. A stale placement is a
 *  guess and is never highlighted. */
export function showResultFromPlacement(p: Placement): ShowElementResult {
	switch (p.state) {
		case "anchored":
			return { tier: "anchored", highlighted: p.at !== null, element: p.at };
		case "reanchored":
			return { tier: "reanchored", highlighted: p.at !== null, element: p.at };
		case "stale":
			return {
				tier: "stale",
				highlighted: false,
				element: p.at,
				...(p.score !== undefined ? { score: p.score } : {}),
			};
		case "lost":
			return { tier: "not_found", highlighted: false, element: null };
	}
}

// ---- arg parsers ----------------------------------------------------------

const reqString = (r: Record<string, unknown>, key: string): string | null =>
	typeof r[key] === "string" && r[key] ? (r[key] as string) : null;

export function parsePanesArgs(raw: unknown): RequestResult<{ roomId: string }> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "bad_arguments: args must be an object" };
	const roomId = reqString(r, "roomId");
	if (!roomId) return { ok: false, error: "bad_arguments: roomId is required" };
	return { ok: true, value: { roomId } };
}

export type DesignTarget = { roomId: string; harnessId: string };

export function parseStateArgs(raw: unknown): RequestResult<DesignTarget> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "bad_arguments: args must be an object" };
	const roomId = reqString(r, "roomId");
	if (!roomId) return { ok: false, error: "bad_arguments: roomId is required" };
	const harnessId = reqString(r, "harnessId");
	if (!harnessId) return { ok: false, error: "bad_arguments: harnessId is required" };
	return { ok: true, value: { roomId, harnessId } };
}

export function parseOpenEntryArgs(raw: unknown): RequestResult<DesignTarget & { entry: string }> {
	const t = parseStateArgs(raw);
	if (!t.ok) return t;
	const entry = (raw as Record<string, unknown>).entry;
	if (typeof entry !== "string" || !entry)
		return { ok: false, error: "bad_arguments: entry is required" };
	return { ok: true, value: { ...t.value, entry } };
}

/** `device` is required but may be null (clear); its content is checked by
 *  `validateDevice`. */
export function parseSetDeviceArgs(
	raw: unknown,
): RequestResult<DesignTarget & { device: object | null }> {
	const t = parseStateArgs(raw);
	if (!t.ok) return t;
	const device = (raw as Record<string, unknown>).device;
	if (device === null || (typeof device === "object" && !Array.isArray(device))) {
		return { ok: true, value: { ...t.value, device } };
	}
	return { ok: false, error: "bad_arguments: device must be an object or null" };
}

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** The agent-facing anchor: at least a selector or a tag; the rest is
 *  optional and defaulted to what `matchElement` tolerates. */
export function parseAnchor(v: unknown): ElementAnchor | null {
	const r = asRecord(v);
	if (!r) return null;
	const selector = typeof r.selector === "string" ? r.selector : "";
	const tag = typeof r.tag === "string" ? r.tag : "";
	if (!selector && !tag) return null;
	if (!isOmitted(r.text) && typeof r.text !== "string") return null;
	if (!isOmitted(r.odId) && typeof r.odId !== "string") return null;
	const attrs: Record<string, string> = {};
	if (!isOmitted(r.attrs)) {
		const ar = asRecord(r.attrs);
		if (!ar) return null;
		for (const [k, x] of Object.entries(ar)) {
			if (typeof x !== "string") return null;
			attrs[k] = x;
		}
	}
	const out: ElementAnchor = {
		selector,
		tag,
		text: typeof r.text === "string" ? r.text : "",
		attrs,
		rect: { x: 0, y: 0, w: 0, h: 0 },
		entry: "",
	};
	if (typeof r.odId === "string" && r.odId) out.odId = r.odId;
	const src = asRecord(r.source);
	if (src && typeof src.file === "string" && isNum(src.line)) {
		out.source = { file: src.file, line: src.line };
	}
	return out;
}

/** `reveal` is opt-in and boolean; only `true` is carried forward. */
export function parseReveal(r: Record<string, unknown>): RequestResult<true | undefined> {
	if (isOmitted(r.reveal)) return { ok: true, value: undefined };
	if (typeof r.reveal !== "boolean") {
		return { ok: false, error: "bad_arguments: reveal must be a boolean" };
	}
	return { ok: true, value: r.reveal ? true : undefined };
}

export function parseShowElementArgs(
	raw: unknown,
): RequestResult<DesignTarget & ShowElementRequest> {
	const t = parseStateArgs(raw);
	if (!t.ok) return t;
	const r = raw as Record<string, unknown>;
	const out: DesignTarget & ShowElementRequest = { ...t.value };
	const reveal = parseReveal(r);
	if (!reveal.ok) return reveal;
	if (reveal.value) out.reveal = true;
	if (!isOmitted(r.selector)) {
		if (typeof r.selector !== "string" || !r.selector) {
			return { ok: false, error: "bad_arguments: selector must be a non-empty string" };
		}
		out.selector = r.selector;
	}
	if (!isOmitted(r.anchor)) {
		const anchor = parseAnchor(r.anchor);
		if (!anchor) {
			return {
				ok: false,
				error: "bad_arguments: anchor must be an object with a selector or tag string",
			};
		}
		out.anchor = anchor;
	}
	if (out.selector === undefined && out.anchor === undefined) {
		return { ok: false, error: "bad_arguments: selector or anchor is required" };
	}
	return { ok: true, value: out };
}
