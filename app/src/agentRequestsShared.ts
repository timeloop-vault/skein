// #330/#411: what every `skein://agent-request` kind's parser shares —
// the result type, the loose-JSON helpers and the attribution shape.
// Pure and DOM-free. `agentRequests.ts` re-exports the public pieces.

import { HARNESS_ORDER } from "./data.tsx";
import type { HarnessKind } from "./types.ts";

export type RequestResult<T> = { ok: true; value: T } | { ok: false; error: string };

export const isHarnessKind = (value: unknown): value is HarnessKind =>
	typeof value === "string" && (HARNESS_ORDER as readonly string[]).includes(value);

export const asRecord = (raw: unknown): Record<string, unknown> | null =>
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
export const isOmitted = (value: unknown): boolean => value === undefined || value === null;

/** `{roomId, harnessId?}` — the attribution shape shared by
 *  `close_room`'s `closedBy` (see `CloseRoomAttribution` below,
 *  structurally identical), `open_harness`'s `createdBy`, and
 *  `close_harness`'s `closedBy`. `harnessId` follows the same
 *  "absent means omitted" rule as everywhere else (`isOmitted`'s doc
 *  comment) — Rust sends JSON `null` for a `None`, not a missing key. */
export type AgentAttribution = { roomId: string; harnessId?: string };

export function parseAttribution(raw: unknown, field: string): RequestResult<AgentAttribution> {
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
