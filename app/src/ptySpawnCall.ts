import { type Channel, invoke } from "@tauri-apps/api/core";
import type { PtySpawnResult } from "./ptySpawnResult.ts";
import type { HarnessKind } from "./types.ts";

export type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

export interface PtySpawnArgs {
	cmd: string[];
	cwd: string;
	rows: number;
	cols: number;
	roomId: string;
	harnessId: string;
	kind: HarnessKind;
	/** #615: the harness's override; undefined = follow the global one. */
	envProfile: string | undefined;
	onEvent: Channel<PtyEvent>;
}

/** The `pty_spawn` invoke. Split out of useTerminalSpawn.ts (400-line cap). */
export function ptySpawn(a: PtySpawnArgs): Promise<PtySpawnResult> {
	return invoke<PtySpawnResult>("pty_spawn", {
		cmd: a.cmd,
		cwd: a.cwd,
		rows: a.rows,
		cols: a.cols,
		// #213: the backend mints the room's review token and puts
		// SKEIN_REVIEW_URL / _TOKEN / SKEIN_ROOM_ID / SKEIN_HARNESS_ID into the
		// child's environment. The harness id is what makes an agent's reply
		// attributable to one harness rather than to "an agent".
		roomId: a.roomId,
		harnessId: a.harnessId,
		// #215: which agent CLI this is, so the backend can append
		// `--plugin-dir` / set `OPENCODE_CONFIG`. Sent as the harness *kind*
		// rather than inferred from `cmd`, because the two legitimately
		// disagree once the user has swapped a command (post-exit shell).
		kind: a.kind,
		// #615: the harness's override; null = follow the global one.
		envProfile: a.envProfile ?? null,
		onEvent: a.onEvent,
	});
}
