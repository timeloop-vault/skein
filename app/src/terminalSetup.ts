// terminalSetup — builds the xterm.js `Terminal` + `FitAddon` pair
// `useTerminalSpawn`'s mount effect wires up, plus the addons and URI
// click handling that go with it. Split out of LiveTerminal.tsx purely
// to keep that effect's file under the ~600-line house limit (#19); no
// behaviour change.

import { openUrl } from "@tauri-apps/plugin-opener";
import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { isMac } from "./shortcuts.ts";

/** Creates and opens a `Terminal` into `host`, wired with the fit addon,
 *  the Unicode 11 width table (#23), and Cmd/Ctrl-click URI opening
 *  (#24). Mirrors exactly what `LiveTerminal`'s mount effect used to do
 *  inline — see the comments below, moved verbatim. */
export function createXterm(
	host: HTMLDivElement,
	fontSize: number,
): { term: Terminal; fit: FitAddon } {
	const term = new Terminal({
		fontFamily: '"JetBrains Mono", ui-monospace, SFMono-Regular, Menlo, monospace',
		fontSize,
		theme: {
			background: "#131418",
			foreground: "#e8e6df",
			cursor: "#c96442",
			selectionBackground: "#20232a",
		},
		cursorBlink: true,
		scrollback: 5000,
		allowProposedApi: true,
		// #158: Claude Code / opencode enable mouse reporting, so a
		// plain drag goes to the TUI instead of an xterm selection.
		// xterm's SelectionService already forces a selection on
		// Shift+drag on Windows/Linux (`shouldForceSelection`, no
		// option needed); on macOS the equivalent is Option+drag, gated
		// behind this option (default false).
		macOptionClickForcesSelection: true,
	});
	const fit = new FitAddon();
	term.loadAddon(fit);
	// Issue #23: Ink-based TUIs (Claude Code, opencode) compute
	// display widths using the `string-width` package, which uses
	// Unicode 11+ tables. xterm.js's default width calculation
	// uses Unicode 6 (circa 2010), so chars like → / ✓ / various
	// arrows are width-2 to Ink but width-1 to xterm. Every such
	// char drifts the cursor by one column, and Ink's incremental
	// streaming redraws stack on top of each other instead of
	// landing where the previous frame's content was — text and
	// dividers leak through. Submitting forces a full clear + redraw
	// so it self-heals on send, which is the smoking gun.
	// The Unicode 11 addon ships an updated wcwidth table that
	// matches what string-width sees. activeVersion = "11" engages
	// it (default stays "6" otherwise — addon must be both loaded
	// AND activated).
	term.loadAddon(new Unicode11Addon());
	term.unicode.activeVersion = "11";

	// Issue #24: Cmd-click (macOS) / Ctrl-click (Win/Linux) opens
	// URIs in the OS-registered handler. Two paths:
	//
	// 1. Plain-text URIs that appear in terminal output (e.g. claude
	//    printing a docs URL, gh printing a PR-create URL). Detected
	//    by WebLinksAddon's regex matcher. The default regex only
	//    catches http(s); we widen it to any well-formed
	//    `scheme://...`, so vscode://, slack://, file://, ssh://,
	//    etc. all work. The OS picks the handler app, so we don't
	//    need a per-scheme allow-list. The leading word-boundary +
	//    ASCII-only scheme guard avoids matching things like
	//    `std::vector` (`vector` isn't a valid URI authority).
	//
	//    The regex has *no* `g` flag — WebLinkProvider always
	//    appends `g` to whatever flags we pass (see
	//    @xterm/addon-web-links/src/WebLinkProvider.ts:60), so
	//    a global flag here would produce `'gg'` and throw
	//    SyntaxError. Letting it own the global flag matches its
	//    contract.
	//
	//    `mailto:` and other no-`://` URIs aren't supported via
	//    this provider — the addon's `isUrl` validator requires
	//    the matched text to start with `protocol://host`, and
	//    `mailto:` URLs have empty hosts. A custom link provider
	//    would be needed; deferred.
	//
	// 2. OSC 8 hyperlinks — the explicit terminal escape
	//    `\e]8;;url\e\\text\e]8;;\e\\` that `gh`, `ls --hyperlink`,
	//    etc. emit. Wired via `term.options.linkHandler`. xterm's
	//    default would call `window.open` which Tauri's WebKit
	//    refuses, so without this OSC 8 links are dead.
	//
	// Modifier check matches the rest of Skein: Cmd on mac, Ctrl
	// elsewhere (see shortcuts.ts). Without modifier, the click
	// falls through to xterm's normal selection behaviour.
	const uriRegex = /\b[a-zA-Z][a-zA-Z0-9+.-]+:\/\/[^\s()[\]{}"'<>\\^`|]+/;
	const handleUriClick = (event: MouseEvent, uri: string) => {
		const isModifierClick = isMac ? event.metaKey : event.ctrlKey;
		if (!isModifierClick) return;
		event.preventDefault();
		void openUrl(uri).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn("[skein] openUrl failed:", uri, msg);
		});
	};
	term.loadAddon(new WebLinksAddon(handleUriClick, { urlRegex: uriRegex }));
	term.options.linkHandler = {
		activate: (event, uri) => handleUriClick(event, uri),
	};

	term.open(host);
	// If we're mounting into a hidden pane (e.g. an inactive room
	// at app boot), skip the initial fit. xterm's defaults (24×80) are
	// what we'll spawn the PTY with; the ResizeObserver tick that
	// fires when the pane becomes visible will refit and pty_resize.
	if (host.clientWidth > 0 && host.clientHeight > 0) {
		fit.fit();
	}
	return { term, fit };
}
