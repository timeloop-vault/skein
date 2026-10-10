// Pure decisions behind useHarnessNotifications (#459): which phase
// transitions are notify-worthy, how a transition is worded, and the
// rooms/toasts reductions the badge and clear-on-view effects apply.
// No React, no Tauri — table-testable.

import {
	type ActivityPhase,
	TRANSITION_SOURCE,
	type TransitionSource,
} from "./harnessActivityTypes.ts";
import type { ToastEntry } from "./toastStack.ts";
import type { Room } from "./types.ts";

export type ToastState = "idle" | "exited" | "waiting" | "permission";

export type TransitionClass = {
	becameWaiting: boolean;
	becamePermission: boolean;
	wasWorking: boolean;
	becamePassive: boolean;
};

// Three trigger classes:
// • `running|idle → waiting` — a harness-native adapter (L2c) reported the
//   agent went from doing work to awaiting user input. Notify-worthy
//   regardless of hasUserInput, since "Claude is now blocked on you" is
//   real news even for a freshly-spawned harness (e.g. first-launch trust
//   prompts). `spawning → waiting` is excluded: that's the synthetic
//   initial-state transition the adapter emits when probing the JSONL on
//   attach. Pre-existing waiting state isn't a notification — it was true
//   before Skein started and the blue dot itself conveys it. Without this
//   gate every Skein restart would badge every Claude room that was
//   sitting at a prompt before shutdown.
// • `* → permission` (#86) — a harness is now blocked on a permission
//   dialog. Notify-worthy unconditionally, same as becameWaiting and for
//   the same reason: there's no replayed permission on boot (Claude's
//   PermissionRequest hook only fires live, mid-session; opencode's
//   pending sets start empty on every fresh connect), so there's no
//   spawning-exclusion case to guard against here.
// • working → passive (running|spawning → idle|exited). The pre-L2c
//   surface: agent went quiet. The hasUserInput gate (applied by the
//   caller) keeps the spawn-banner cycle on every Skein restart from
//   lighting up every room.
export function classifyTransition(from: ActivityPhase, to: ActivityPhase): TransitionClass {
	return {
		becameWaiting: to === "waiting" && (from === "running" || from === "idle"),
		becamePermission: to === "permission",
		wasWorking: from === "running" || from === "spawning",
		becamePassive: to === "idle" || to === "exited",
	};
}

// #175: an opencode reconnect baseline is a guess at idle, never news.
export function isNotifiable(c: TransitionClass, source?: TransitionSource): boolean {
	if (source === TRANSITION_SOURCE.L2c2OpencodeBaseline) return false;
	return c.becameWaiting || c.becamePermission || (c.wasWorking && c.becamePassive);
}

// "waiting"/"permission" wording surfaces the L2c case in toast / OS banner
// so the user knows the agent wants something from them — not that it
// finished. The ToastEntry's `state` field flows into the existing toast
// component, which renders "permission" as "needs permission" (+ tool)
// rather than verbatim (#86).
export function toastState(to: ActivityPhase): ToastState {
	return to === "waiting"
		? "waiting"
		: to === "permission"
			? "permission"
			: to === "idle"
				? "idle"
				: "exited";
}

export function osNotificationLabel(
	state: ToastState,
	delegationNote: string | null,
	permissionAgentType: string | null,
	permissionTool: string | null,
): string {
	const parts = [permissionAgentType, permissionTool].filter((p): p is string => p !== null);
	return state === "permission"
		? `needs permission${parts.length > 0 ? ` (${parts.join(" · ")})` : ""}`
		: state === "waiting" && delegationNote
			? `${state} (${delegationNote})`
			: state;
}

export function newToastId(): string {
	return `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
}

// pendingNotifications is a capped boolean (#159): 0 or 1.
export function setHarnessPending(rooms: Room[], harnessId: string): Room[] {
	return rooms.map((r) => {
		if (!r.harnesses.some((h) => h.id === harnessId)) return r;
		return {
			...r,
			harnesses: r.harnesses.map((h) =>
				h.id === harnessId ? { ...h, pendingNotifications: 1 } : h,
			),
		};
	});
}

export function clearHarnessPending(rooms: Room[], roomId: string, harnessId: string): Room[] {
	return rooms.map((r) => {
		if (r.id !== roomId) return r;
		const target = r.harnesses.find((h) => h.id === harnessId);
		if (!target || (target.pendingNotifications ?? 0) === 0) return r;
		return {
			...r,
			harnesses: r.harnesses.map((h) =>
				h.id === harnessId ? { ...h, pendingNotifications: 0 } : h,
			),
		};
	});
}

// Return the same array when nothing matches so we don't trigger a
// needless re-render.
export function dropToastsFor(
	toasts: ToastEntry[],
	roomId: string,
	harnessId: string,
): ToastEntry[] {
	const next = toasts.filter((t) => !(t.roomId === roomId && t.harnessId === harnessId));
	return next.length === toasts.length ? toasts : next;
}
