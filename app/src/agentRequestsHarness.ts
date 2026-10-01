import {
	type AgentAttribution,
	asRecord,
	isHarnessKind,
	isOmitted,
	parseAttribution,
	type RequestResult,
} from "./agentRequestsShared.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";

// ── open_harness.resolve / open_harness ────────────────────────────
//
// #411: `open_harness` reuses `resolveKind`/`resolveAgent` unchanged —
// the same folder-memory defaulting `create_room.resolve` runs, just
// against an existing room's own `cwd` rather than a fresh folder path
// (see `useAgentRequests.ts`'s `handleOpenResolve`, which looks the
// room up and passes its `cwd` through). `decideOpenHarness` is the
// only new refusal ladder here: an unknown or already-archived target
// room. Two things the contract also assigns this verb live Rust-side
// on purpose and are NOT reimplemented here: the `room_full` ceiling
// (no frontend state needed — the harness count Rust already has is
// enough) and the "can this kind read mail" check for a `prompt`
// (`mail_refusal_for` in `verbs.rs`, which needs injection settings and
// the agent's own tool allowlist — both Rust-only knowledge). Both are
// applied Rust-side once it has this round trip's `{kind, agent}` back,
// the same way `create_room` already applies the mail check to
// `create_room.resolve`'s answer rather than asking the frontend to.

export interface OpenHarnessResolveArgs {
	roomId: string;
	kind?: string;
	agent?: string;
	prompt: boolean;
}

/** Parse+validate the raw JSON `args` of an `open_harness.resolve`
 *  request. `prompt` is a plain `bool` on the Rust side (not an
 *  `Option`), so unlike `kind`/`agent` it is never "omitted". */
export function parseOpenHarnessResolveArgs(raw: unknown): RequestResult<OpenHarnessResolveArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) return { ok: false, error: "roomId is required" };
	if (!isOmitted(r.kind) && typeof r.kind !== "string") {
		return { ok: false, error: "kind must be a string" };
	}
	if (!isOmitted(r.agent) && typeof r.agent !== "string") {
		return { ok: false, error: "agent must be a string" };
	}
	if (typeof r.prompt !== "boolean") return { ok: false, error: "prompt must be a boolean" };
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			...(typeof r.kind === "string" ? { kind: r.kind } : {}),
			...(typeof r.agent === "string" ? { agent: r.agent } : {}),
			prompt: r.prompt,
		},
	};
}

export interface OpenHarnessArgs {
	roomId: string;
	kind: HarnessKind;
	/** Already resolved (and validated) by a prior
	 *  `open_harness.resolve` round trip — `null` means "the tool's own
	 *  default", same meaning it has everywhere else. */
	agent: string | null;
	createdBy: AgentAttribution;
}

/** Parse+validate the raw JSON `args` of an `open_harness` request. */
export function parseOpenHarnessArgs(raw: unknown): RequestResult<OpenHarnessArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) return { ok: false, error: "roomId is required" };
	if (!isHarnessKind(r.kind)) {
		return { ok: false, error: `unknown harness kind "${String(r.kind)}"` };
	}
	if (r.agent !== null && typeof r.agent !== "string") {
		return { ok: false, error: "agent must be a string or null" };
	}
	const createdBy = parseAttribution(r.createdBy, "createdBy");
	if (!createdBy.ok) return createdBy;
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			kind: r.kind,
			agent: r.agent === null ? null : r.agent,
			createdBy: createdBy.value,
		},
	};
}

export interface OpenHarnessDecision {
	room: Room;
}

/** The `open_harness` (and `.resolve`) refusal ladder: unknown room,
 *  then already archived. Pure and shared by both round trips'
 *  handlers in `useAgentRequests.ts` — resolve needs the room's `cwd`,
 *  open needs to know it's still safe to add to. */
export function decideOpenHarness(
	rooms: readonly Room[],
	roomId: string,
): RequestResult<OpenHarnessDecision> {
	const room = rooms.find((r) => r.id === roomId);
	if (!room) return { ok: false, error: `not_found: room "${roomId}" not found` };
	if (room.archived) return { ok: false, error: `archived: room "${roomId}" is already archived` };
	return { ok: true, value: { room } };
}

// ── close_harness ─────────────────────────────────────────────────
//
// #411: same shape as `close_room` above — Rust has already checked
// scope/rate-limit by the time this request lands, so what's left is
// the refusal ladder an agent close (never a confirm dialog) can still
// hit: unknown harness, the room's last harness (that's `close_room`'s
// job), an open permission dialog, then unsaved Files buffers — in
// that order, matching the contract. `closedBy` is parsed for shape
// parity with the wire payload but never stored: unlike `Room.closedBy`
// there is no `Harness.closedBy` field — Rust attributes the close in
// its own `harness_actions` row instead.

export interface CloseHarnessArgs {
	roomId: string;
	harnessId: string;
	closedBy: AgentAttribution;
}

export function parseCloseHarnessArgs(raw: unknown): RequestResult<CloseHarnessArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) {
		return { ok: false, error: "roomId is required" };
	}
	if (typeof r.harnessId !== "string" || !r.harnessId) {
		return { ok: false, error: "harnessId is required" };
	}
	const closedBy = parseAttribution(r.closedBy, "closedBy");
	if (!closedBy.ok) return closedBy;
	return {
		ok: true,
		value: { roomId: r.roomId, harnessId: r.harnessId, closedBy: closedBy.value },
	};
}

export interface CloseHarnessDecision {
	room: Room;
	harness: Harness;
	/** The phase the harness was in immediately before the close — what
	 *  the verb replies with. `"unknown"` when this process can't vouch
	 *  for it (`phaseFor` returned nothing for it), the same fallback
	 *  every phase read in the agent API promises. */
	phase: string;
}

/** The `close_harness` refusal ladder, pure so it's testable without a
 *  rooms hook or a real `filesRegistry`/`harnessActivity` — `phaseFor`
 *  and `dirtyNamesFor` stand in for `harnessActivity.phaseSnapshot()`
 *  and `filesRegistry.dirtyNames`, injected the same way
 *  `decideCloseRoom`'s `dirtyNamesFor` is. */
export function decideCloseHarness(
	rooms: readonly Room[],
	roomId: string,
	harnessId: string,
	phaseFor: (harnessId: string) => string | undefined,
	dirtyNamesFor: (harnessId: string) => string[],
): RequestResult<CloseHarnessDecision> {
	const room = rooms.find((r) => r.id === roomId);
	if (!room) return { ok: false, error: `not_found: room "${roomId}" not found` };
	const harness = room.harnesses.find((h) => h.id === harnessId);
	if (!harness) return { ok: false, error: `not_found: harness "${harnessId}" not found` };
	if (room.harnesses.length === 1) {
		return {
			ok: false,
			error: `last_harness: "${harnessId}" is the only harness in room "${roomId}" — use close_room instead`,
		};
	}
	const phase = phaseFor(harnessId) ?? "unknown";
	if (phase === "permission") {
		return {
			ok: false,
			error: `permission_open: harness "${harnessId}" has an open permission dialog`,
		};
	}
	const dirty = dirtyNamesFor(harnessId);
	if (dirty.length > 0) return { ok: false, error: `unsaved_files: ${dirty.join(", ")}` };
	return { ok: true, value: { room, harness, phase } };
}
