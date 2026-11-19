// #269, narrowed by #571 and #401: whether Skein itself should act on a
// Cmd/Ctrl-clicked terminal link, or defer because the foreground CLI
// already owns the click (see `HarnessCapabilities.opensClickedLinks` in
// data.tsx for the full story — Claude Code's fullscreen renderer opens
// the URL itself once mouse tracking is on, so both opening it would show
// two tabs).
//
// Deferring is only safe when the CLI really receives the click and acts:
// - #571: on macOS the modifier is Cmd, and xterm's mouse protocol has no
//   Cmd bit, so the CLI sees a plain click and opens nothing. Never defer.
// - #401: with `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` truthy in the harness
//   env the CLI ignores clicks (tracking stays on for scrolling). Never
//   defer.
// - #813: a link in this build's own deep-link scheme (`skein://room/...`)
//   is different. The CLI can only hand it back to the OS, and on Windows
//   Claude Code doesn't, so Skein always owns it: it opens on a modifier
//   click, and also on a plain click while mouse tracking is on (a plain
//   click then never starts a selection, so nothing else would use it).
//   On Linux a CLI that opens clicked links (Claude Code) also gets the
//   plain click and may open the same link too; opening a room twice is
//   idempotent, so this is accepted.

import { invoke } from "@tauri-apps/api/core";

/** Plain-text URIs WebLinksAddon turns into links (#24): any well-formed
 *  `scheme://…`, not only http(s), so the OS picks the handler. The leading
 *  word boundary and ASCII-only scheme keep `std::vector` out.
 *
 *  #706: the last character can't be sentence punctuation (`.,;:!?`), so
 *  "see https://x.dev/a." or "https://x.dev/a, and" open `https://x.dev/a`.
 *  The same set file paths drop (`TRAILING` in terminalPathText.ts); a URL
 *  that really ends in one of those loses it, as in other terminals.
 *
 *  No `g` flag: WebLinkProvider appends one itself, and `gg` throws. */
export const URI_REGEX =
	/\b[a-zA-Z][a-zA-Z0-9+.-]+:\/\/[^\s()[\]{}"'<>\\^`|]*[^\s()[\]{}"'<>\\^`|.,;:!?]/;

/** Lowercased scheme of `scheme://…`, or null when there is none. */
export function uriScheme(uri: string): string | null {
	const i = uri.indexOf("://");
	if (i <= 0) return null;
	return uri.slice(0, i).toLowerCase();
}

/** True when `uri` is in one of this build's own schemes. */
export function isOwnSchemeLink(uri: string, schemes: readonly string[]): boolean {
	const scheme = uriScheme(uri);
	return scheme !== null && schemes.some((s) => s.toLowerCase() === scheme);
}

let ownSchemes: readonly string[] = [];
let ownSchemesLoad: Promise<void> | null = null;

/** Loads this build's link schemes from Rust, once. Failure leaves the list empty. */
export function loadOwnLinkSchemes(): Promise<void> {
	ownSchemesLoad ??= invoke<string[]>("open_request_link_schemes")
		.then((schemes) => {
			ownSchemes = schemes;
		})
		.catch((err: unknown) => {
			console.warn("[skein] open_request_link_schemes failed:", err);
			ownSchemesLoad = null;
		});
	return ownSchemesLoad;
}

/** The loaded schemes; empty until `loadOwnLinkSchemes` resolves. */
export function ownLinkSchemes(): readonly string[] {
	return ownSchemes;
}

export interface LinkPolicyInput {
	isMac: boolean;
	/** `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` is truthy in the harness env. */
	mouseClicksDisabled: boolean;
	mouseTrackingOn: boolean;
	cliOpensClickedLinks: boolean;
	/** The link is in this build's own scheme (#813). */
	ownScheme: boolean;
	/** The Cmd (mac) / Ctrl click modifier was held. */
	modifierHeld: boolean;
}

/** True when Skein should open the link itself. Own scheme: on a modifier
 *  click, or a plain click with mouse tracking on. Any other link needs the
 *  modifier, and Skein defers only when the CLI will provably open it: not
 *  macOS, clicks not disabled, mouse tracking on, and the CLI opens clicked
 *  links itself. */
export function shouldHostOpenLink({
	isMac,
	mouseClicksDisabled,
	mouseTrackingOn,
	cliOpensClickedLinks,
	ownScheme,
	modifierHeld,
}: LinkPolicyInput): boolean {
	if (ownScheme) return modifierHeld || mouseTrackingOn;
	if (!modifierHeld) return false;
	return isMac || mouseClicksDisabled || !mouseTrackingOn || !cliOpensClickedLinks;
}
