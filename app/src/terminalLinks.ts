// #269: whether Skein itself should act on a Cmd/Ctrl-clicked terminal
// link, or defer because the foreground CLI already owns the click (see
// `HarnessCapabilities.opensClickedLinks` in data.tsx for the full
// story — Claude Code's fullscreen renderer opens the URL itself once
// mouse tracking is on, so both opening it too would show two tabs).

/** True when Skein should open the link itself. False only when the
 *  foreground app has mouse tracking on, its CLI is known to open
 *  clicked links on its own, AND the click's modifier actually reaches
 *  the CLI — in that case the CLI owns the click, and Skein opening it
 *  too would be a second, duplicate tab.
 *
 *  `modifierReachesCli` is false on macOS: xterm's mouse protocol
 *  encodes Ctrl/Alt/Shift but has no bit for Cmd
 *  (`CoreBrowserTerminal.ts` forwards `ctrlKey`/`altKey`/`shiftKey`
 *  only), so a Cmd-click arrives as a plain click, which Claude Code
 *  does not treat as a link open. Deferring there opened nothing. */
export function shouldHostOpenLink(
	mouseTrackingOn: boolean,
	cliOpensClickedLinks: boolean,
	modifierReachesCli: boolean,
): boolean {
	return !(mouseTrackingOn && cliOpensClickedLinks && modifierReachesCli);
}
