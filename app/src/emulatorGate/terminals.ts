// Headless xterm.js terminals configured the way the client's are
// (terminalSetup.ts): scrollback 5000, Unicode 11 widths, proposed API on.

import { Unicode11Addon } from "@xterm/addon-unicode11";
import { Terminal } from "@xterm/headless";

/** The client's `scrollback` option (terminalSetup.ts). */
export const CLIENT_SCROLLBACK = 5000;

export interface GateTerm {
	term: Terminal;
}

export function makeTerm(cols: number, rows: number): GateTerm {
	const term = new Terminal({
		cols,
		rows,
		scrollback: CLIENT_SCROLLBACK,
		allowProposedApi: true,
	});
	// The Unicode11 addon is typed against @xterm/xterm's Terminal; the
	// headless one is structurally what it touches (unicode.register).
	term.loadAddon(new Unicode11Addon() as unknown as Parameters<Terminal["loadAddon"]>[0]);
	term.unicode.activeVersion = "11";
	return { term };
}

export function write(term: Terminal, data: Uint8Array | string): Promise<void> {
	return new Promise((res) => term.write(data, res));
}
