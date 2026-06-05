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
