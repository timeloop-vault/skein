// The context-limit nudge's text rules (#689). The daemon sends the nudge;
// this is only the editing side of `SpawnSettings.contextNudge`.

/** The text used when no custom body is saved. Keep identical to the Rust
 *  twin: `DEFAULT_BODY` in crates/skein-daemon/src/prompt_delivery/context_nudge.rs. */
export const DEFAULT_CONTEXT_NUDGE_BODY =
	"Your context is at {tokens} tokens, over the limit of {limit} set in Skein. Tell the user it is time to hand over or start a fresh session. Do not compact or restart on your own.";

/** Wire shape inside SpawnSettings. Absent = `{ enabled: true, body: null }`. */
export interface ContextNudgeSettings {
	enabled: boolean;
	/** null (or blank) = the default text. */
	body: string | null;
}

export const DEFAULT_CONTEXT_NUDGE: ContextNudgeSettings = { enabled: true, body: null };

/** The text the daemon will send for a stored body. */
export function effectiveBody(body: string | null | undefined): string {
	return body === null || body === undefined || body.trim() === ""
		? DEFAULT_CONTEXT_NUDGE_BODY
		: body;
}

/** What to store for an edited text: blank or equal to the default is null. */
export function bodyToStore(edit: string): string | null {
	if (edit.trim() === "") return null;
	if (edit === DEFAULT_CONTEXT_NUDGE_BODY) return null;
	return edit;
}

/** True when a custom body is in effect. */
export function isCustomBody(body: string | null | undefined): boolean {
	return effectiveBody(body) !== DEFAULT_CONTEXT_NUDGE_BODY;
}
