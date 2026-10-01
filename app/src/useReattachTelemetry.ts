import { type MutableRefObject, useCallback } from "react";
import { reattachClaudeTelemetry } from "./harnessEvents.ts";
import { reattachOutcomeMessage } from "./reattachOutcomeMessage.ts";
import type { Room } from "./types.ts";
import type { useHarnessNotifications } from "./useHarnessNotifications.ts";

type PushToast = ReturnType<typeof useHarnessNotifications>["pushToast"];

// #410: manual "Reattach telemetry" action (HarnessActionsMenu's
// menu item + its command palette twin, both gated on
// `hasClaudeTranscriptTail`) for when the badge is visibly wrong
// because the Rust-side Claude JSONL tail died silently. Just an
// invoke + a toast reporting what happened — the phase itself
// settles on its own from the events a genuine reattach picks back
// up, same as any other attach.
export function useReattachTelemetry(roomsRef: MutableRefObject<Room[]>, pushToast: PushToast) {
	return useCallback(
		(roomId: string, harnessId: string) => {
			const owningRoom = roomsRef.current.find((r) => r.id === roomId);
			const harness = owningRoom?.harnesses.find((h) => h.id === harnessId);
			if (!owningRoom || !harness) return;
			const toast = (message: string) =>
				pushToast({
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owningRoom.id,
					harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: "info",
					message,
				});
			void reattachClaudeTelemetry(harnessId)
				.then((outcome) => toast(reattachOutcomeMessage({ ok: true, outcome })))
				.catch((err: unknown) => {
					const msg = err instanceof Error ? err.message : String(err);
					toast(reattachOutcomeMessage({ ok: false, error: msg }));
				});
		},
		[roomsRef, pushToast],
	);
}
