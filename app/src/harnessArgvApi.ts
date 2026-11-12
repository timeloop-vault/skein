// invoke wrappers for the daemon's harness argv commands (#730). The argv
// itself is built in crates/skein-daemon/src/harness_argv/; the frontend
// only asks. Results merge back through harnessArgvApply.ts, never wholesale.

import { invoke } from "@tauri-apps/api/core";
import type { Harness, HarnessKind, RemoteSpec, Room } from "./types.ts";

export interface FreshCmdInput {
	kind: HarnessKind;
	sessionId?: string;
	/** A brand-new harness: mint a Claude session id when none is given AND
	 *  default the Hermes profile to Skein's own. Absent/false (repoint) passes
	 *  `sessionId` and `hermesProfile` through as given. */
	newHarness?: boolean;
	agent?: string;
	remote?: RemoteSpec;
	hermesProfile?: string;
}

export interface FreshCmdResult {
	/** Empty for kinds without a process (files/design) or an unusable remote. */
	cmd: string[];
	/** null = no port (not opencode, or the allocation failed: a bare argv). */
	opencodePort: number | null;
	sessionId: string | null;
	hermesProfile: string | null;
}

export type FreshCmdFn = (input: FreshCmdInput) => Promise<FreshCmdResult>;

export const harnessFreshCmd: FreshCmdFn = (input) =>
	invoke<FreshCmdResult>("harness_fresh_cmd", { input });

export interface ResumedHarness {
	harness: Harness;
	/** null for opencode = allocation failed; other kinds always null. */
	opencodePort: number | null;
}

export const harnessResumeCmd = (harness: Harness): Promise<ResumedHarness> =>
	invoke<ResumedHarness>("harness_resume_cmd", { input: { harness } });

export interface ResumedRooms {
	rooms: Room[];
	ports: Record<string, number>;
}

export const roomsResumeCmds = (rooms: Room[], unarchive = false): Promise<ResumedRooms> =>
	invoke<ResumedRooms>("rooms_resume_cmds", {
		input: { rooms, ...(unarchive ? { unarchive } : {}) },
	});
