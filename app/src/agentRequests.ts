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
import type { HarnessKind } from "./types.ts";
import type { CreateRoomSpec } from "./worktreeRoom.ts";

export type RequestResult<T> = { ok: true; value: T } | { ok: false; error: string };

const isHarnessKind = (value: unknown): value is HarnessKind =>
	typeof value === "string" && (HARNESS_ORDER as readonly string[]).includes(value);

const asRecord = (raw: unknown): Record<string, unknown> | null =>
	typeof raw === "object" && raw !== null && !Array.isArray(raw)
		? (raw as Record<string, unknown>)
		: null;

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
	if (r.kind !== undefined && typeof r.kind !== "string") {
		return { ok: false, error: "kind must be a string" };
	}
	if (r.agent !== undefined && typeof r.agent !== "string") {
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
	if (r.branch !== undefined && typeof r.branch !== "string") {
		return { ok: false, error: "branch must be a string" };
	}
	if (r.baseBranch !== undefined && typeof r.baseBranch !== "string") {
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
		},
	};
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
