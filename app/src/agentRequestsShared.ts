// What every `skein://agent-request` kind's parser shares (the design.*
// requests, since #734) — the result type and the loose-JSON helpers.
// Pure and DOM-free.

export type RequestResult<T> = { ok: true; value: T } | { ok: false; error: string };

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
