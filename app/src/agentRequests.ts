// #330: argument parsing, default resolution and response shaping for
// the two `skein://agent-request` kinds the `create_room` agent verb
// sends — `create_room.resolve` and `create_room` — plus the two
// existing pieces they lean on (#247's agent validation, #227's branch
// template). Pure and DOM-free so vitest can cover it without a Tauri
// bridge; `useAgentRequests.ts` is the only place that actually invokes
// anything or touches `rooms` state, and it's a thin dispatcher over
// this module.
//
// Mirrors the New Room dialog's own defaulting (`useNewRoomForm.tsx`)
// rather than reinventing it, so a room opened by an agent looks exactly
// like one a human would have gotten for the same folder: kind falls
// back to the folder's own remembered harness, else `"claude"`
// (`useNewRoomForm.tsx`'s `initialDefaults?.harness ?? "claude"`); agent
// falls back through `startingAgent` (prefs.ts) the same way.

import { type AgentListing, unknownAgentMessage, validateAgent } from "./agents.ts";
import { HARNESS_ORDER } from "./data.tsx";
import { type DefaultAgents, type FolderDefaults, startingAgent } from "./prefs.ts";
import type { Harness, HarnessKind, Room } from "./types.ts";
import type { CreateRoomSpec } from "./worktreeRoom.ts";

export type RequestResult<T> = { ok: true; value: T } | { ok: false; error: string };

const isHarnessKind = (value: unknown): value is HarnessKind =>
	typeof value === "string" && (HARNESS_ORDER as readonly string[]).includes(value);

const asRecord = (raw: unknown): Record<string, unknown> | null =>
	typeof raw === "object" && raw !== null && !Array.isArray(raw)
		? (raw as Record<string, unknown>)
		: null;

/** `true` for an optional field that was left out entirely. Rust's
 *  `serde_json::json!` macro serializes an absent `Option<String>` as
 *  JSON `null`, never as a missing key, so every optional-string check
 *  below has to treat the two as the same "not given" — the bug this
 *  guards against shipped live: a request with only the required
 *  fields still carried `kind: null, agent: null, ...` and was rejected
 *  as "kind must be a string". */
const isOmitted = (value: unknown): boolean => value === undefined || value === null;

/** `{roomId, harnessId?}` — the attribution shape shared by
 *  `close_room`'s `closedBy` (see `CloseRoomAttribution` below,
 *  structurally identical), `open_harness`'s `createdBy`, and
 *  `close_harness`'s `closedBy`. `harnessId` follows the same
 *  "absent means omitted" rule as everywhere else (`isOmitted`'s doc
 *  comment) — Rust sends JSON `null` for a `None`, not a missing key. */
export type AgentAttribution = { roomId: string; harnessId?: string };

function parseAttribution(raw: unknown, field: string): RequestResult<AgentAttribution> {
	const r = asRecord(raw);
	if (!r || typeof r.roomId !== "string" || !r.roomId) {
		return { ok: false, error: `${field} must be {roomId, harnessId?}` };
	}
	if (!isOmitted(r.harnessId) && typeof r.harnessId !== "string") {
		return { ok: false, error: `${field}.harnessId must be a string` };
	}
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			...(typeof r.harnessId === "string" ? { harnessId: r.harnessId } : {}),
		},
	};
}

// ── create_room.resolve ──────────────────────────────────────────────

export interface ResolveRequestArgs {
	path: string;
	kind?: string;
	agent?: string;
}

/** Parse+validate the raw JSON `args` of a `create_room.resolve`
 *  request. Deliberately permissive about what it does NOT require —
 *  `kind`/`agent` are optional exactly because "not given" is a
 *  meaningful input (means "resolve the default"), not a missing field. */
export function parseResolveArgs(raw: unknown): RequestResult<ResolveRequestArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.path !== "string" || !r.path.trim()) return { ok: false, error: "path is required" };
	if (!isOmitted(r.kind) && typeof r.kind !== "string") {
		return { ok: false, error: "kind must be a string" };
	}
	if (!isOmitted(r.agent) && typeof r.agent !== "string") {
		return { ok: false, error: "agent must be a string" };
	}
	return {
		ok: true,
		value: {
			path: r.path,
			...(typeof r.kind === "string" ? { kind: r.kind } : {}),
			...(typeof r.agent === "string" ? { agent: r.agent } : {}),
		},
	};
}

/** Kind resolution: `requested` if it names a real `HarnessKind`, else
 *  the folder's own remembered harness, else `"claude"` — same fallback
 *  chain `useNewRoomForm.tsx` seeds its `harness` state with. */
export function resolveKind(
	requested: string | undefined,
	folderDefaults: FolderDefaults | undefined,
): RequestResult<HarnessKind> {
	if (requested === undefined) return { ok: true, value: folderDefaults?.harness ?? "claude" };
	if (!isHarnessKind(requested)) return { ok: false, error: `unknown harness kind "${requested}"` };
	return { ok: true, value: requested };
}

/** Agent resolution + #247 validation for one kind. `requested` wins
 *  over the folder/default-agent fallback chain (`startingAgent`), same
 *  as a name typed into the picker overrides what it was prefilled
 *  with. `unknown` is the only verdict that blocks — a degraded listing
 *  (`unverified`) never does, matching every other agent-picking call
 *  site in the app (`validateAgent`'s own doc explains why: refusing on
 *  a CLI that's briefly unavailable would strand every harness). The
 *  resolved name comes back as `string | null`, never `undefined` — the
 *  wire shape `create_room.resolve` answers with. */
export function resolveAgent(
	requested: string | undefined,
	kind: HarnessKind,
	folderDefaults: FolderDefaults | undefined,
	defaultAgents: DefaultAgents,
	listing: AgentListing,
): RequestResult<string | null> {
	const candidate = requested ?? startingAgent(folderDefaults, kind, defaultAgents);
	const verdict = validateAgent(candidate, listing);
	if (verdict.kind === "unknown") {
		return { ok: false, error: unknownAgentMessage(verdict.agent, kind) };
	}
	return { ok: true, value: candidate?.trim() ? candidate : null };
}

// ── create_room ───────────────────────────────────────────────────────

export interface CreateRequestArgs {
	path: string;
	branchMode: "worktree" | "current";
	branch?: string;
	baseBranch?: string;
	task: string;
	kind: HarnessKind;
	/** Already resolved (and validated) by a prior `create_room.resolve`
	 *  round trip — `null` means "the tool's own default", same meaning
	 *  it has everywhere else in the app. */
	agent: string | null;
	createdBy: { roomId: string; harnessId: string };
	/** The calling room's name, for the receipt toast — passed straight
	 *  through rather than looked up here, so this module never needs
	 *  `rooms` state to shape a response. */
	requesterRoomName: string;
	/** #356: the first mailbox message queued for the new harness, when
	 *  one was given (`docs/agent-api.md`'s `create_room` `prompt` arg).
	 *  Only ever used here to derive `promptFirstLine` below — the
	 *  message itself is queued Rust-side, not by this round trip. */
	prompt?: string;
	/** #356: Rust's own first-line extraction, when it already computed
	 *  one. Wins over deriving one from `prompt` — see
	 *  `derivePromptFirstLine`. */
	promptFirstLine?: string;
}

/** #356: `Room.createdBy.promptFirstLine` — a director's `list_rooms`
 *  needs a title for a room that was never given a `name`, and the
 *  Room record doesn't store the prompt itself. `explicit` (Rust's own
 *  extraction) wins outright when it isn't blank; otherwise the first
 *  non-empty trimmed line of `prompt` is used. `undefined` when
 *  neither yields anything — an unset field, never an empty string, to
 *  match the rest of this module's "absent means absent" convention. */
export const PROMPT_FIRST_LINE_MAX = 200;

export function derivePromptFirstLine(
	explicit: string | undefined,
	prompt: string | undefined,
): string | undefined {
	if (explicit?.trim()) return explicit.trim().slice(0, PROMPT_FIRST_LINE_MAX);
	const line = prompt?.split(/\r?\n/).find((l) => l.trim().length > 0);
	return line ? line.trim().slice(0, PROMPT_FIRST_LINE_MAX) : undefined;
}

/** Parse+validate the raw JSON `args` of a `create_room` request. Unlike
 *  `parseResolveArgs`, every field here is required — by the time this
 *  kind is sent, `create_room.resolve` has already filled in `kind` and
 *  `agent`, so an absent one is a caller bug worth refusing loudly
 *  rather than re-defaulting silently and maybe defaulting twice. */
export function parseCreateArgs(raw: unknown): RequestResult<CreateRequestArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.path !== "string" || !r.path.trim()) return { ok: false, error: "path is required" };
	if (r.branchMode !== "worktree" && r.branchMode !== "current") {
		return { ok: false, error: 'branchMode must be "worktree" or "current"' };
	}
	if (!isOmitted(r.branch) && typeof r.branch !== "string") {
		return { ok: false, error: "branch must be a string" };
	}
	if (!isOmitted(r.baseBranch) && typeof r.baseBranch !== "string") {
		return { ok: false, error: "baseBranch must be a string" };
	}
	if (typeof r.task !== "string" || !r.task.trim()) return { ok: false, error: "task is required" };
	if (!isHarnessKind(r.kind))
		return { ok: false, error: `unknown harness kind "${String(r.kind)}"` };
	if (r.agent !== null && typeof r.agent !== "string") {
		return { ok: false, error: "agent must be a string or null" };
	}
	// `harnessId` may be `""` — the caller's own `X-Skein-Harness` header
	// is optional, and Rust sends the empty string rather than `null` in
	// that case (same fallback `send_message`'s `harness_actions` rows
	// use). Only `roomId` has to be non-empty: it is what every other
	// verb in this API scopes on, and an empty one can never resolve.
	const createdByRaw = asRecord(r.createdBy);
	if (
		!createdByRaw ||
		typeof createdByRaw.roomId !== "string" ||
		!createdByRaw.roomId ||
		typeof createdByRaw.harnessId !== "string"
	) {
		return { ok: false, error: "createdBy must be {roomId, harnessId}" };
	}
	if (typeof r.requesterRoomName !== "string" || !r.requesterRoomName.trim()) {
		return { ok: false, error: "requesterRoomName is required" };
	}
	if (!isOmitted(r.prompt) && typeof r.prompt !== "string") {
		return { ok: false, error: "prompt must be a string" };
	}
	if (!isOmitted(r.promptFirstLine) && typeof r.promptFirstLine !== "string") {
		return { ok: false, error: "promptFirstLine must be a string" };
	}
	return {
		ok: true,
		value: {
			path: r.path,
			branchMode: r.branchMode,
			...(typeof r.branch === "string" ? { branch: r.branch } : {}),
			...(typeof r.baseBranch === "string" ? { baseBranch: r.baseBranch } : {}),
			task: r.task,
			kind: r.kind,
			agent: r.agent === null ? null : r.agent,
			createdBy: { roomId: createdByRaw.roomId, harnessId: createdByRaw.harnessId },
			requesterRoomName: r.requesterRoomName,
			...(typeof r.prompt === "string" ? { prompt: r.prompt } : {}),
			...(typeof r.promptFirstLine === "string" ? { promptFirstLine: r.promptFirstLine } : {}),
		},
	};
}

// ── close_room ──────────────────────────────────────────────────────
//
// #411: the frontend half of the `close_room` agent verb — Rust has
// already checked the creator/sign-off/rate-limit rules by the time
// this request lands, so all that's left here is the same refusal
// ladder the user's own close already has to clear implicitly (a room
// that doesn't exist or is already archived can't be closed, and
// dirty Files buffers can't be discarded without asking — except an
// agent call has no one to ask, so it refuses instead).

export interface CloseRoomAttribution {
	roomId: string;
	harnessId?: string;
}

export interface CloseRoomArgs {
	roomId: string;
	closedBy: CloseRoomAttribution;
}

/** Parse+validate the raw JSON `args` of a `close_room` request.
 *  `closedBy.harnessId` follows the same "absent means omitted" rule
 *  as `create_room`'s `createdBy` (see `isOmitted`'s doc comment) —
 *  Rust sends JSON `null` for a `None`, not a missing key. */
export function parseCloseRoomArgs(raw: unknown): RequestResult<CloseRoomArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) {
		return { ok: false, error: "roomId is required" };
	}
	const closedByRaw = asRecord(r.closedBy);
	if (!closedByRaw || typeof closedByRaw.roomId !== "string" || !closedByRaw.roomId) {
		return { ok: false, error: "closedBy must be {roomId, harnessId?}" };
	}
	if (!isOmitted(closedByRaw.harnessId) && typeof closedByRaw.harnessId !== "string") {
		return { ok: false, error: "closedBy.harnessId must be a string" };
	}
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			closedBy: {
				roomId: closedByRaw.roomId,
				...(typeof closedByRaw.harnessId === "string" ? { harnessId: closedByRaw.harnessId } : {}),
			},
		},
	};
}

export interface CloseRoomDecision {
	room: Room;
	closedBy: NonNullable<Room["closedBy"]>;
}

/** The `close_room` refusal ladder, pure so it's testable without a
 *  rooms hook or a real `filesRegistry`: unknown room, then already
 *  archived, then unsaved Files buffers — in that order, since naming
 *  dirty files in a room that doesn't exist or is already closed would
 *  be a strange error to receive. `dirtyNamesFor` stands in for
 *  `filesRegistry.anyDirty`, injected rather than imported so a test
 *  doesn't have to register/unregister real registry entries. On
 *  success, `closedBy` is the stamped attribution ready to write onto
 *  the room — `at` filled in from `now` here rather than by the
 *  caller, so every accepted close is stamped the same way. */
export function decideCloseRoom(
	rooms: readonly Room[],
	roomId: string,
	closedBy: CloseRoomAttribution,
	dirtyNamesFor: (harnessIds: string[]) => string[],
	now: number,
): RequestResult<CloseRoomDecision> {
	const room = rooms.find((r) => r.id === roomId);
	if (!room) return { ok: false, error: `not_found: room "${roomId}" not found` };
	if (room.archived) return { ok: false, error: `archived: room "${roomId}" is already archived` };
	const dirty = dirtyNamesFor(room.harnesses.map((h) => h.id));
	if (dirty.length > 0) return { ok: false, error: `unsaved_files: ${dirty.join(", ")}` };
	return {
		ok: true,
		value: {
			room,
			closedBy: {
				roomId: closedBy.roomId,
				...(closedBy.harnessId ? { harnessId: closedBy.harnessId } : {}),
				at: now,
			},
		},
	};
}

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

/** `CreateRequestArgs` → the `CreateRoomSpec` `worktreeRoom.ts`'s
 *  `createRoomArgs` takes — the same translation `useNewRoomForm.tsx`'s
 *  `submit()` does inline for the dialog's own fields. */
export function specFromCreateArgs(
	args: CreateRequestArgs,
	branchTemplate: string,
): CreateRoomSpec {
	return {
		folder: args.path,
		task: args.task,
		harness: args.kind,
		...(args.agent ? { agent: args.agent } : {}),
		branchMode: args.branchMode,
		...(args.branch ? { branch: args.branch } : {}),
		...(args.baseBranch ? { baseBranch: args.baseBranch } : {}),
		branchTemplate,
	};
}
