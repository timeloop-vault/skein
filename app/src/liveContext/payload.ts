// Shared payload accessors for the Activity rows. A harness_actions
// row's `payload` is an opaque JSON string whose shape varies by kind
// (and, for shared kinds, by harness) — see
// docs/live-context-d2-buildmap.md. These keep the row components
// defensive: a missing/wrong-typed field reads as undefined rather
// than throwing.

export type Payload = Record<string, unknown>;

/// Parse a row's `payload` JSON string. Returns {} on malformed input
/// or a non-object top level, so callers can always index safely.
export function parsePayload(raw: string): Payload {
	try {
		const v: unknown = JSON.parse(raw);
		return v && typeof v === "object" ? (v as Payload) : {};
	} catch {
		return {};
	}
}

export const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
export const num = (v: unknown): number | undefined => (typeof v === "number" ? v : undefined);
export const obj = (v: unknown): Payload | undefined =>
	v && typeof v === "object" ? (v as Payload) : undefined;

/// A name-or-id pair, both possibly absent (#329 mailbox rows carry
/// snake_case strings, any of which the sender/recipient may not have
/// had to hand). Prefers the human name, falls back to the id, and "?"
/// when neither is present.
function nameOrId(name: unknown, id: unknown): string {
	return str(name) ?? str(id) ?? "?";
}

/// The `message_in` row's gist: "message from <room> · <harness>",
/// dropping the " · <harness>" clause entirely when no harness name or
/// id survived (rather than rendering a bare "·").
export function messageInLabel(payload: Payload): string {
	const room = nameOrId(payload.from_room_name, payload.from_room_id);
	const harness = str(payload.from_harness_label) ?? str(payload.from_harness_id);
	return harness ? `message from ${room} · ${harness}` : `message from ${room}`;
}

/// The `message_out` row's gist: "message to <room> · <harness>", same
/// fallback rule as messageInLabel.
export function messageOutLabel(payload: Payload): string {
	const room = nameOrId(payload.to_room_name, payload.to_room_id);
	const harness = str(payload.to_harness_label) ?? str(payload.to_harness_id);
	return harness ? `message to ${room} · ${harness}` : `message to ${room}`;
}
