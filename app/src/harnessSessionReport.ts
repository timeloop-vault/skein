// Since #755 the harness session id is daemon-owned. The client reports what
// it observed through `harness_session_report`; the changed record arrives
// back through skein://rooms-changed (origin "daemon-session"), never by
// writing the field locally.

import { invoke } from "@tauri-apps/api/core";

export type SessionReport =
	| { mode: "capture"; sessionId: string }
	| { mode: "replace"; sessionId: string }
	| { mode: "clear" };

/** Resolves true when the stored record changed; false for an unknown harness
 *  or nothing to change. */
export const reportHarnessSession = (
	roomId: string,
	harnessId: string,
	report: SessionReport,
): Promise<boolean> => invoke<boolean>("harness_session_report", { roomId, harnessId, report });
