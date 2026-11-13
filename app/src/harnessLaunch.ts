// What a harness's pane runs (#752). `Harness.cmd` is always the TOOL's
// argv; the post-exit shell is a separate `launch` mode on the record.

import type { Harness, LaunchMode } from "./types.ts";

/** The record's launch mode; absent means the tool. */
export const launchOf = (h: Pick<Harness, "launch">): LaunchMode => h.launch ?? { mode: "tool" };

/** The argv the pane actually runs: the shell's in shell mode, else `cmd`. */
export const spawnArgv = (h: Pick<Harness, "cmd" | "launch">): string[] | undefined =>
	h.launch?.mode === "shell" ? h.launch.argv : h.cmd;
