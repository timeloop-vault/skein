// #269: whether Skein itself should act on a Cmd/Ctrl-clicked terminal
// link, or defer because the foreground CLI already owns the click (see
// `HarnessCapabilities.opensClickedLinks` in data.tsx for the full
// story — Claude Code's fullscreen renderer opens the URL itself once
// mouse tracking is on, so both opening it too would show two tabs).

/** True when Skein should open the link itself. False when the
 *  foreground app has mouse tracking on AND its CLI is known to open
 *  clicked links on its own — in that case the CLI owns the click, and
 *  Skein opening it too would be a second, duplicate tab. */
export function shouldHostOpenLink(
	mouseTrackingOn: boolean,
	cliOpensClickedLinks: boolean,
): boolean {
	return !(mouseTrackingOn && cliOpensClickedLinks);
}
