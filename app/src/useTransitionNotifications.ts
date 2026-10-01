// The harnessActivity transition → badge / toast / OS notification
// effect (L5a/L5b/L5c), split out of useHarnessNotifications (#459; no
// behaviour change). The pure predicates and wording live in
// harnessNotifyLogic.ts.
//
// L5a — pending-notification accounting. A harness transitioning
// from working (spawning|running) to passive (idle|exited), or
// into `permission` (#86), bumps its own `pendingNotifications`
// counter — unless it's the harness the user is currently viewing
// (active room's active harness), in which case we skip because
// the user can already see the dot change. Same harness in the
// active room but in a non-active harness tab WILL bump — its tab
// isn't visible. Room.badge is rendered as the sum across
// harnesses by RoomStrip.tsx's LiveRoomTab / GroupTab; we don't write to
// it here. Counters persist via the rooms→sqlite mirror so the
// badge survives a restart.
//
// L5b — OS notification. Same predicates as the badge bump
// (passive/permission transition + not the viewed harness +
// hasUserInput where required), PLUS Skein is not the focused app.
// If Skein is focused the badge update is already visible and an
// OS banner would just duplicate it. Permission is granted lazily
// on first launch.

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect } from "react";
import { backgroundTasks } from "./backgroundTasks.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { harnessActivity, TRANSITION_SOURCE } from "./harnessActivity.ts";
import { waitingNote } from "./harnessActivityLabels.ts";
import {
	classifyTransition,
	isNotifiable,
	newToastId,
	osNotificationLabel,
	setHarnessPending,
	toastState,
} from "./harnessNotifyLogic.ts";
import { BADGE_COALESCE_MS, enqueueOsNotification } from "./notifications.tsx";
import { appendToast, type NewToast, type ToastEntry } from "./toastStack.ts";
import type { Room } from "./types.ts";
import type { NotificationPermission } from "./useWindowFocusPermission.ts";

export function useTransitionNotifications(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setToasts: Dispatch<SetStateAction<ToastEntry[]>>,
	notifyBadgeRef: MutableRefObject<boolean>,
	notifyToastRef: MutableRefObject<boolean>,
	notifyOsRef: MutableRefObject<boolean>,
	lastBadgeAtRef: MutableRefObject<Map<string, number>>,
	windowFocusedRef: MutableRefObject<boolean>,
	notificationPermissionRef: MutableRefObject<NotificationPermission>,
) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			const cls = classifyTransition(from, to);
			if (!isNotifiable(cls)) return;
			const a = harnessActivity.get(harnessId);
			if (!a) return;
			// #277: the end-of-turn notification explains itself when it
			// was withheld for delegated work — but only when Skein can
			// still claim the delegation "finished". The `DelegationCeiling`
			// path flushes because a subagent signal was presumed lost
			// after 15 min of silence, not because the work actually
			// completed, so no suffix rides along there; every other route
			// into `waiting` (including `DelegationSettled`) can say so
			// honestly. #441: a ceiling/watchdog flush instead says how many
			// background tasks are presumed still running, never "finished".
			// Only a ceiling/watchdog flush consumes the overdue tasks, so an
			// ordinary waiting never swallows them and a later notice never
			// repeats ones already announced.
			const flushed =
				source === TRANSITION_SOURCE.DelegationCeiling || source === TRANSITION_SOURCE.WorkWatchdog;
			const delegationNote =
				to === "waiting"
					? waitingNote(
							source,
							a.delegatedCount,
							flushed ? backgroundTasks.takeUnreportedOverdue(harnessId) : 0,
						)
					: null;
			// hasUserInput gate applies only to the passive transition.
			// `→ waiting` and `→ permission` are both unconditional.
			if (!cls.becameWaiting && !cls.becamePermission && !a.hasUserInput) return;
			const activeRoom = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
			const isViewedHarness = Boolean(activeRoom && activeRoom.activeHarnessId === harnessId);
			const isWindowFocused = windowFocusedRef.current;
			const owningRoom = roomsRef.current.find((r) => r.harnesses.some((h) => h.id === harnessId));
			// #127: shells aren't agents — an idle/exited shell is just a
			// prompt sitting there (or an `exit` you typed), never
			// notification-worthy, and L2a idle-timeout / prompt-redraw
			// chatter made them pop up spuriously. Suppress every surface
			// (badge / toast / OS) for kinds without the notify capability
			// (byoh, files); the status dot still reflects running/idle.
			// Agents (claude/opencode/copilot) notify as before.
			const kind = owningRoom?.harnesses.find((h) => h.id === harnessId)?.kind;
			if (kind && !HARNESS_KINDS[kind].capabilities.notify) return;
			// Badge: skip when the user is staring at this exact
			// harness — the tab dot color change tells them what
			// happened. If they alt+tabbed away, though, bump
			// anyway so they see "something happened while I was
			// gone" when they come back. Also skip when the badge
			// surface is disabled in Settings (L5e).
			//
			// pendingNotifications is a capped boolean (#159): 0 or 1, so
			// no transition can push it past 1 (it previously accumulated
			// unbounded — a room hit 38). The coalesce window (#62/#64)
			// additionally skips redundant state updates when a burst of
			// badge-worthy transitions lands within BADGE_COALESCE_MS.
			// Record the time on every badge-worthy transition (skipped or
			// not) so continuous sub-window chatter never re-triggers a set.
			const nowMs = Date.now();
			const curPending =
				owningRoom?.harnesses.find((h) => h.id === harnessId)?.pendingNotifications ?? 0;
			const lastBadgeAt = lastBadgeAtRef.current.get(harnessId) ?? 0;
			const coalesced = curPending > 0 && nowMs - lastBadgeAt < BADGE_COALESCE_MS;
			lastBadgeAtRef.current.set(harnessId, nowMs);
			if (notifyBadgeRef.current && !(isViewedHarness && isWindowFocused) && !coalesced) {
				setRooms((prev) => setHarnessPending(prev, harnessId));
			}
			const harness = owningRoom?.harnesses.find((h) => h.id === harnessId);
			const kindName = harness ? HARNESS_KINDS[harness.kind].name : "harness";
			const stateLabel = toastState(to);
			// L5c — in-app toast. Fires when window IS focused but
			// the user isn't looking at the source harness (they're
			// in Skein, but in a different room or different tab).
			// Skipped when window is unfocused (OS notification
			// handles that case), when viewing the harness (badge
			// dot + tab color already tell the story), or when
			// disabled in Settings (L5e).
			if (notifyToastRef.current && isWindowFocused && !isViewedHarness && owningRoom && harness) {
				const entry: NewToast = {
					id: newToastId(),
					roomId: owningRoom.id,
					harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: stateLabel,
					...(stateLabel === "permission"
						? { tool: a.permissionTool ?? undefined, agentType: a.permissionAgentType ?? undefined }
						: {}),
					...(delegationNote ? { delegationNote } : {}),
				};
				setToasts((prev) => appendToast(prev, entry, Date.now()));
			}
			// OS notification — fire whenever the window isn't
			// focused, regardless of which harness was "viewed"
			// inside Skein. The user alt+tabbed away; they need
			// the OS-level signal to know to come back. When
			// focused, the badge update is already on screen and
			// an OS banner would just duplicate it. Also gated on
			// the per-surface Settings toggle (L5e).
			if (isWindowFocused) return;
			if (!notifyOsRef.current) return;
			if (notificationPermissionRef.current !== "granted") return;
			if (!owningRoom) return;
			// Serialized through the module-level chain (#84) so concurrent
			// transitions never call the plugin's `show` at the same time.
			// The helper also catches plugin-absent rejections (dev builds
			// skip it — see app/src-tauri/src/lib.rs).
			const osLabel = osNotificationLabel(
				stateLabel,
				delegationNote,
				a.permissionAgentType,
				a.permissionTool,
			);
			enqueueOsNotification("Skein", `${owningRoom.name} · ${kindName}: ${osLabel}`, {
				roomId: owningRoom.id,
				harnessId,
			});
		});
		return unsub;
	}, []);
}
