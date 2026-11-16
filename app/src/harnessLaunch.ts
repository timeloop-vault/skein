// What a harness's pane runs (#752). `Harness.cmd` is always the TOOL's
// argv; the post-exit shell is a separate `launch` mode on the record.

import type { Harness, LaunchMode } from "./types.ts";

/** The record's launch mode; absent means the tool. */
export const launchOf = (h: Pick<Harness, "launch">): LaunchMode => h.launch ?? { mode: "tool" };

/** The argv the pane actually runs: the shell's in shell mode, else `cmd`. */
export const spawnArgv = (h: Pick<Harness, "cmd" | "launch">): string[] | undefined =>
	h.launch?.mode === "shell" ? h.launch.argv : h.cmd;

/** Whether the pane runs its tool, so the tool's own transcript is live (#763).
 *  False only in the post-exit shell with no claim. Mirrors the daemon's
 *  `runs_its_tool` in harness_argv/launch.rs. */
export const runsItsTool = (h: Pick<Harness, "launch" | "shellClaim">): boolean =>
	launchOf(h).mode !== "shell" || h.shellClaim !== undefined;
