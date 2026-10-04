// Control Center pop-out contract (#493), shared by the main window and the
// pop-out window. Pure types and constants, no imports with side effects.
//
// Roles. The MAIN window owns every state store (harnessActivity,
// subagents, backgroundTasks live only in its webview), so it keeps
// computing the rows and broadcasts them; the pop-out is a thin renderer
// of `ControlCenterView` over the last snapshot it received.
//
// Events (Tauri events, so payloads are JSON):
//   cc-popout:snapshot  main -> pop-out   CcSnapshotPayload. Throttled by
//                       main; always sent in reply to `ready`.
//   cc-popout:ready     pop-out -> main   no payload. Emit once your
//                       listeners are attached; main answers with a fresh
//                       snapshot. Target it with emitTo("main", ...).
//   cc-popout:focus     pop-out -> main   CcFocusPayload. Main activates the
//                       room (and harness), then raises the main window.
//
// `sections` is the output of buildControlCenter() verbatim. It is plain
// data (rooms are the persisted Room JSON, snapshots are numbers/strings),
// so it survives JSON; `undefined` fields simply disappear, which every
// consumer already treats as absent. Timestamps are epoch ms.

import type { CCSection } from "./model.ts";

/** Window label of the pop-out (also its capability scope). */
export const POPOUT_LABEL = "control-center";

export const EV_SNAPSHOT = "cc-popout:snapshot";
export const EV_READY = "cc-popout:ready";
export const EV_FOCUS = "cc-popout:focus";

export interface CcSnapshotPayload {
	sections: CCSection[];
	activeRoomId: string | null;
	/** The instant every relative time in `sections` is measured from. */
	now: number;
}

export interface CcFocusPayload {
	roomId: string;
	harnessId?: string;
}
