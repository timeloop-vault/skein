// The embedded-server port of an opencode harness (#731). The daemon bakes a
// freshly allocated `--port <n>` into each opencode cmd at boot, so the cmd
// is where the SSE attach finds it; there is no separate ports map at boot.

import type { Room } from "./types.ts";

/** The `--port` value in `cmd` (`--port 4097` or `--port=4097`), or null. */
export function opencodePortFromCmd(cmd: readonly string[]): number | null {
	for (let i = 0; i < cmd.length; i++) {
		const a = cmd[i];
		if (a === undefined) continue;
		const raw = a === "--port" ? cmd[i + 1] : a.startsWith("--port=") ? a.slice(7) : undefined;
		if (raw === undefined || !/^\d+$/.test(raw)) continue;
		const n = Number(raw);
		if (n > 0 && n <= 65535) return n;
	}
	return null;
}

/** harness id -> port for every opencode harness whose cmd carries one. */
export function opencodePortsFromRooms(rooms: readonly Room[]): Map<string, number> {
	const ports = new Map<string, number>();
	for (const r of rooms) {
		for (const h of r.harnesses) {
			if (h.kind !== "opencode") continue;
			const port = opencodePortFromCmd(h.cmd ?? []);
			if (port !== null) ports.set(h.id, port);
		}
	}
	return ports;
}
