// Harness argv helpers still living in the frontend. The argv for creation,
// repoint, restart, boot and unarchive is built by the daemon (#730,
// crates/skein-daemon/src/harness_argv/), reached through harnessArgvApi.ts.
// The opencode v2 decision and argv are the daemon's too (#730, harness_ensure_running).
// What remains here is `isOpencodeV2` (non-argv decisions) and the Hermes
// default profile name.

import type { OpencodeVersion } from "./types.ts";

/** Skein's own Hermes profile (#626); the default for new Hermes harnesses
 *  (the daemon resolves it at creation; the picker preselects it). */
export const SKEIN_HERMES_PROFILE = "skein";

/** Major version 2 or later. Null / unknown reads as v1 (#675). */
export const isOpencodeV2 = (version: OpencodeVersion | null | undefined): boolean =>
	(version?.major ?? 0) >= 2;
