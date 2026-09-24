// The notification engine, extracted out of App.tsx (#19) — pure move,
// no behaviour change. Owns the toast stack (`toasts`/`dismissToast`/
// `jumpToToast`), the window-focus + OS-notification-permission refs,
// the `harness-permission` and `harness-session-start` Tauri event
// listeners, the `harnessActivity.subscribeTransitions` badge/toast/OS
// notification effect (L5a/L5b/L5c), the api_error → toast effect
// (D2f), the `db_record_harness_event` transition-logging effect (L6),
// and the clear-pending-on-view effect. NOT here: the OS-notification
// click handling (`jumpToTarget`/`drainPendingClick`) — that stays in
// App.tsx as a separate, later extraction. App.tsx calls this hook at
// the point this state used to be declared and destructures the
// return value the same way it does `useRoomsStore`/`useHarnessCreation`.

import {
	isPermissionGranted,
	requestPermission,
} from "@choochmeque/tauri-plugin-notifications-api";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect, useRef, useState } from "react";
import { HARNESS_KINDS } from "./data.tsx";
import { TRANSITION_SOURCE, delegationSummary, harnessActivity } from "./harnessActivity.ts";
import {
	ACTION_EVENT,
	type HarnessAction,
	apiErrorToastText,
	parsePayload,
} from "./liveContext/index.ts";
import {
	API_ERROR_INCIDENT_MS,
	BADGE_COALESCE_MS,
	TOAST_MAX_VISIBLE,
	type ToastEntry,
	enqueueOsNotification,
} from "./notifications.tsx";
import { clearAttention } from "./roomAttention.ts";
import { followedSession } from "./sessionTracking.ts";
import type { Room } from "./types.ts";

export function useHarnessNotifications(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	activeRoomId: string,
	displayedHarnessId: string | null,
	notifyBadge: boolean,
	notifyToast: boolean,
	notifyOs: boolean,
	replaceHarnessSessionId: (targetRoomId: string, harnessId: string, sessionId: string) => void,
	jumpToHarness: (roomId: string, harnessId: string) => void,
) {
	// L5c — in-app toasts. Ephemeral (no DB mirror) since they
	// represent "right now, look here" state that doesn't survive
	// a restart. Capped at TOAST_MAX_VISIBLE so a burst of
	// transitions doesn't cover the screen.
	const [toasts, setToasts] = useState<ToastEntry[]>([]);

	const dismissToast = (id: string) => {
		setToasts((prev) => prev.filter((t) => t.id !== id));
	};

	const jumpToToast = (toast: ToastEntry) => {
		jumpToHarness(toast.roomId, toast.harnessId);
		dismissToast(toast.id);
	};

	// L5e — notification toggles read inside the transition listener
	// (mounted once with empty deps); refs let preference toggles
	// take effect without re-subscribing.
	const notifyBadgeRef = useRef(notifyBadge);
	notifyBadgeRef.current = notifyBadge;
	const notifyToastRef = useRef(notifyToast);
	notifyToastRef.current = notifyToast;
	const notifyOsRef = useRef(notifyOs);
	notifyOsRef.current = notifyOs;
	// D2f — last api_error arrival per harness, for coalescing a retry
	// burst into one badge-worthy incident.
	const lastApiErrorAtRef = useRef<Map<string, number>>(new Map());
	// Per-harness time of the last badge bump, for the coalesce window
	// (#62/#64 — collapse a burst/chatter of transitions into one badge).
	const lastBadgeAtRef = useRef<Map<string, number>>(new Map());

	// L5b — window-focus state + OS-notification permission. The
	// notification logic below skips firing an OS banner when Skein
	// is the focused app, because the user is already looking at
	// the badge update in real time and an extra OS-level banner is
	// just noise. Refs (not state) since the transition callback
	// reads these synchronously and we don't want them to retrigger
	// the subscription effect on every focus change.
	const windowFocusedRef = useRef(true);
	const notificationPermissionRef = useRef<"unknown" | "granted" | "denied">("unknown");
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const win = getCurrentWindow();
		let unlisten: (() => void) | null = null;
		// Clear the badge on the currently-displayed harness. Used
		// both by the (activeRoomId, displayedHarnessId) effect below
		// AND by the focus listener — coming back to Skein on the
		// same harness you alt+tabbed away from also counts as
		// "viewing it now," but that effect doesn't re-fire because
		// neither tuple value changed. Hook the focus→true edge
		// instead.
		const clearDisplayedHarnessPending = () => {
			const room = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
			if (!room) return;
			const displayedH = room.activeHarnessId;
			if (!displayedH) return;
			setRooms((prev) =>
				prev.map((r) => {
					if (r.id !== room.id) return r;
					const target = r.harnesses.find((h) => h.id === displayedH);
					if (!target || (target.pendingNotifications ?? 0) === 0) return r;
					return {
						...r,
						harnesses: r.harnesses.map((h) =>
							h.id === displayedH ? { ...h, pendingNotifications: 0 } : h,
						),
					};
				}),
			);
		};
		void win.isFocused().then((f) => {
			windowFocusedRef.current = f;
		});
		void win
			.onFocusChanged(({ payload }) => {
				windowFocusedRef.current = payload;
				if (payload) clearDisplayedHarnessPending();
			})
			.then((u) => {
				unlisten = u;
			});
		// Permission flow: prompt once on first run if the user
		// hasn't decided yet. The OS remembers the choice and
		// future `isPermissionGranted` calls return granted/denied
		// without re-prompting.
		void (async () => {
			try {
				const granted = await isPermissionGranted();
				if (granted) {
					notificationPermissionRef.current = "granted";
					return;
				}
				const result = await requestPermission();
				notificationPermissionRef.current = result === "granted" ? "granted" : "denied";
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] notification permission flow failed:", msg);
			}
		})();
		return () => {
			unlisten?.();
		};
	}, []);

	// #86: Claude's PermissionRequest hook fires this global event the
	// moment a permission dialog is on screen — the Rust side owns the
	// hook wiring, this is just the frontend half of the contract.
	// Room-agnostic by design: the harness id alone is enough to route
	// it, and `setPermissionFromAdapter` is already a no-op for an id
	// Skein doesn't have a record for (a stale event racing a closed
	// harness).
	useEffect(() => {
		const un = listen<{
			roomId: string;
			harnessId: string;
			toolName: string | null;
			agentType: string | null;
			agentId: string | null;
		}>("skein://harness-permission", (event) => {
			harnessActivity.setPermissionFromAdapter(
				event.payload.harnessId,
				TRANSITION_SOURCE.L2c1ClaudePermission,
				event.payload.toolName,
				event.payload.agentType,
				event.payload.agentId,
			);
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);

	// #273: Claude's `SessionStart` command hook fires this global event
	// the moment its CLI has (re)started — including before any
	// transcript exists, which is exactly the gap that used to leave a
	// freshly spawned harness stuck in `spawning` forever. Room-agnostic
	// like the permission listener above; `noteLaunchSignal` is already
	// a no-op for an id Skein doesn't have a record for.
	//
	// #116: the same hook is the ONLY signal Skein gets that a
	// mid-session `/clear`, in-tool `/resume` or a fork (`/branch`,
	// `/fork`, `--fork-session`, desktop rewind) moved a harness onto a
	// different conversation — Claude's JSONL gives no other sign.
	// `followedSession` (sessionTracking.ts) filters to `source ===
	// "clear"`, `"resume"` or `"fork"` reporting a genuinely different
	// id; when it does, the harness's stored sessionId is overwritten
	// (`replaceHarnessSessionId`, first-writer-wins would never adopt it)
	// so a future resume/reopen picks up the new conversation, and
	// `harnessActivity.sessionSwitched` forgets the old session's
	// subagents/delegation state. Per-pane re-pointing of the live JSONL
	// tail itself happens in LiveTerminal, keyed off the `sessionId`
	// prop — this listener only owns the persisted record. Attribution
	// for which pane the hook fired in comes from `SKEIN_HARNESS_ID` via
	// the `X-Skein-Harness` header, not from anything computed here.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const un = listen<{
			roomId: string;
			harnessId: string;
			sessionId: string | null;
			source: string | null;
		}>("skein://harness-session-start", (event) => {
			harnessActivity.noteLaunchSignal(event.payload.harnessId);
			const room = roomsRef.current.find((r) => r.id === event.payload.roomId);
			const h = room?.harnesses.find((x) => x.id === event.payload.harnessId);
			if (!h || h.kind !== "claude") return;
			const next = followedSession(h.sessionId, event.payload);
			if (next !== null) {
				replaceHarnessSessionId(event.payload.roomId, event.payload.harnessId, next.sessionId);
				harnessActivity.sessionSwitched(event.payload.harnessId, next.source);
			}
		});
		return () => {
			void un.then((f) => f());
		};
	}, []);

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
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			// Three trigger classes:
			// • `running|idle → waiting` — a harness-native adapter
			//   (L2c) reported the agent went from doing work to
			//   awaiting user input. Notify-worthy regardless of
			//   hasUserInput, since "Claude is now blocked on you"
			//   is real news even for a freshly-spawned harness
			//   (e.g. first-launch trust prompts).
			//   `spawning → waiting` is excluded: that's the
			//   synthetic initial-state transition the adapter
			//   emits when probing the JSONL on attach. Pre-existing
			//   waiting state isn't a notification — it was true
			//   before Skein started and the blue dot itself
			//   conveys it. Without this gate every Skein restart
			//   would badge every Claude room that was sitting at
			//   a prompt before shutdown.
			// • `* → permission` (#86) — a harness is now blocked on a
			//   permission dialog. Notify-worthy unconditionally, same
			//   as becameWaiting and for the same reason: there's no
			//   replayed permission on boot (Claude's PermissionRequest
			//   hook only fires live, mid-session; opencode's pending
			//   sets start empty on every fresh connect), so there's no
			//   spawning-exclusion case to guard against here.
			// • working → passive (running|spawning → idle|exited).
			//   The pre-L2c surface: agent went quiet. We keep the
			//   hasUserInput gate so the spawn-banner cycle on
			//   every Skein restart doesn't light up every room.
			const becameWaiting = to === "waiting" && (from === "running" || from === "idle");
			const becamePermission = to === "permission";
			const wasWorking = from === "running" || from === "spawning";
			const becamePassive = to === "idle" || to === "exited";
			if (!becameWaiting && !becamePermission && !(wasWorking && becamePassive)) return;
			const a = harnessActivity.get(harnessId);
			if (!a) return;
			// #277: the end-of-turn notification explains itself when it
			// was withheld for delegated work — but only when Skein can
			// still claim the delegation "finished". The `DelegationCeiling`
			// path flushes because a subagent signal was presumed lost
			// after 15 min of silence, not because the work actually
			// completed, so no suffix rides along there; every other route
			// into `waiting` (including `DelegationSettled`) can say so
			// honestly.
			const delegationNote =
				to === "waiting" && source !== TRANSITION_SOURCE.DelegationCeiling
					? delegationSummary(a.delegatedCount)
					: null;
			// hasUserInput gate applies only to the passive transition.
			// `→ waiting` and `→ permission` are both unconditional.
			if (!becameWaiting && !becamePermission && !a.hasUserInput) return;
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
				setRooms((prev) =>
					prev.map((r) => {
						if (!r.harnesses.some((h) => h.id === harnessId)) return r;
						return {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === harnessId ? { ...h, pendingNotifications: 1 } : h,
							),
						};
					}),
				);
			}
			const harness = owningRoom?.harnesses.find((h) => h.id === harnessId);
			const kindName = harness ? HARNESS_KINDS[harness.kind].name : "harness";
			// "waiting"/"permission" wording surfaces the L2c case in
			// toast / OS banner so the user knows the agent wants
			// something from them — not that it finished. The
			// ToastEntry's `state` field flows into the existing toast
			// component, which renders "permission" as "needs
			// permission" (+ tool) rather than verbatim (#86).
			const stateLabel: "idle" | "exited" | "waiting" | "permission" =
				to === "waiting"
					? "waiting"
					: to === "permission"
						? "permission"
						: to === "idle"
							? "idle"
							: "exited";
			// L5c — in-app toast. Fires when window IS focused but
			// the user isn't looking at the source harness (they're
			// in Skein, but in a different room or different tab).
			// Skipped when window is unfocused (OS notification
			// handles that case), when viewing the harness (badge
			// dot + tab color already tell the story), or when
			// disabled in Settings (L5e).
			if (notifyToastRef.current && isWindowFocused && !isViewedHarness && owningRoom && harness) {
				const entry: ToastEntry = {
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
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
				setToasts((prev) => [...prev, entry].slice(-TOAST_MAX_VISIBLE));
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
			const osPermissionParts = [a.permissionAgentType, a.permissionTool].filter(
				(p): p is string => p !== null,
			);
			const osLabel =
				stateLabel === "permission"
					? `needs permission${osPermissionParts.length > 0 ? ` (${osPermissionParts.join(" · ")})` : ""}`
					: stateLabel === "waiting" && delegationNote
						? `${stateLabel} (${delegationNote})`
						: stateLabel;
			enqueueOsNotification("Skein", `${owningRoom.name} · ${kindName}: ${osLabel}`, {
				roomId: owningRoom.id,
				harnessId,
			});
		});
		return unsub;
	}, []);

	// D2f (#80) — graduated error treatment, steps 2+3 (handover §6).
	// api_error rows don't flow through harnessActivity (they're
	// harness_actions rows), so this dedicated listener feeds the
	// existing notification surfaces: an error-variant toast when the
	// error lands in a room the user isn't looking at, and a
	// pendingNotifications bump so the tab badge + status-bar urgent
	// segment persist until the room gets attention (the stream carries
	// no "resolved" signal — attention is the only clearing semantic).
	// A retry burst is one incident (real data: 4 rows in 11 s) — the
	// badge bumps once per window, and the toast updates in place while
	// it's still showing rather than stacking.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unlistenPromise = listen<HarnessAction>(ACTION_EVENT, (event) => {
			const a = event.payload;
			if (a.kind !== "api_error") return;
			// §6: the inline ApiErrorRow covers the active room; the toast
			// exists for errors the user can't currently see.
			if (a.roomId === activeRoomIdRef.current) return;
			const owningRoom = roomsRef.current.find((r) => r.id === a.roomId);
			const harness = owningRoom?.harnesses.find((h) => h.id === a.harnessId);
			if (!owningRoom || !harness) return;
			const now = Date.now();
			const last = lastApiErrorAtRef.current.get(a.harnessId) ?? 0;
			const newIncident = now - last > API_ERROR_INCIDENT_MS;
			lastApiErrorAtRef.current.set(a.harnessId, now);
			if (notifyBadgeRef.current && newIncident) {
				setRooms((prev) =>
					prev.map((r) => {
						if (!r.harnesses.some((h) => h.id === a.harnessId)) return r;
						return {
							...r,
							harnesses: r.harnesses.map((h) =>
								h.id === a.harnessId ? { ...h, pendingNotifications: 1 } : h,
							),
						};
					}),
				);
			}
			if (!notifyToastRef.current || !windowFocusedRef.current) return;
			const detail = apiErrorToastText(parsePayload(a.payload));
			setToasts((prev) => {
				const i = prev.findIndex((t) => t.state === "error" && t.harnessId === a.harnessId);
				const existing = i === -1 ? undefined : prev[i];
				if (existing) {
					// Coalesce onto the live toast (same id) with fresh detail,
					// so a fast burst is one toast, not a stack. Retries that
					// outpace the 6s dismiss (529 backoff spaces them out:
					// ~0.5/1/2/4s and growing) let the toast lapse between
					// rows, so the next retry re-surfaces a fresh one — the
					// incident keeps re-announcing itself, which is fine; the
					// badge (one bump per incident) is the persistent signal.
					const next = [...prev];
					next[i] = { ...existing, detail };
					return next;
				}
				const entry: ToastEntry = {
					id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
					roomId: owningRoom.id,
					harnessId: a.harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: "error",
					detail,
				};
				return [...prev, entry].slice(-TOAST_MAX_VISIBLE);
			});
		});
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);

	// L6 — append every real phase transition to the sqlite event
	// log. Per-transition fire-and-forget; errors warn but don't
	// surface UX. The log feeds (eventually) the L7 cross-harness
	// activity feed; in the meantime the data exists for any
	// "since last visit" surface to build on. Epic #50 L6.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unsub = harnessActivity.subscribeTransitions((harnessId, from, to, source) => {
			const owningRoom = roomsRef.current.find((r) => r.harnesses.some((h) => h.id === harnessId));
			if (!owningRoom) {
				// Transition for a harness that's no longer in
				// state — e.g. exit firing after the room was
				// archived. Without a roomId we can't usefully log;
				// skip.
				return;
			}
			const activity = harnessActivity.get(harnessId);
			void invoke("db_record_harness_event", {
				harnessId,
				roomId: owningRoom.id,
				fromPhase: from,
				toPhase: to,
				timestampMs: Date.now(),
				hasUserInput: activity?.hasUserInput ?? false,
				// L7a (#73): per-transition attribution.
				// Identifies which strategy fired it (`l2a-idle`,
				// `l2b-pattern`, `l2c1-claude-end-turn`, etc.) for
				// the eventual L7c activity feed.
				source,
			}).catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] db_record_harness_event failed for ${harnessId}:`, msg);
			});
		});
		return unsub;
	}, []);

	// L5a — clear pending on view. Runs every time the (active room,
	// active harness of active room) tuple changes — covers tab
	// click, keyboard nav (Mod+1..9, Mod+Tab), command palette,
	// initial load. Only the harness that's now displayed gets
	// cleared; other harnesses in the same room keep their pending
	// counts so a multi-harness room only loses badges as the user
	// visits each tab.
	// biome-ignore lint/correctness/useExhaustiveDependencies: setRooms comes from useRoomsStore (#19) — a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		// #328: the room-level `attention` mark clears independently of
		// `displayedHarnessId` below — a room becoming active is what
		// "visited" means for it, not which harness inside it happens to
		// be showing.
		setRooms((prev) => clearAttention(prev, activeRoomId));
		if (!activeRoomId || !displayedHarnessId) return;
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== activeRoomId) return r;
				const target = r.harnesses.find((h) => h.id === displayedHarnessId);
				if (!target || (target.pendingNotifications ?? 0) === 0) return r;
				return {
					...r,
					harnesses: r.harnesses.map((h) =>
						h.id === displayedHarnessId ? { ...h, pendingNotifications: 0 } : h,
					),
				};
			}),
		);
		// Landing on a harness means you're now looking at it, so drop any
		// lingering toast for it — regardless of how you got here (Mod+J/L,
		// palette, tab, Mod+1..9). Clicking a toast already dismisses it via
		// jumpToToast; this covers every other path. Return the same array
		// when nothing matches so we don't trigger a needless re-render.
		setToasts((prev) => {
			const next = prev.filter(
				(t) => !(t.roomId === activeRoomId && t.harnessId === displayedHarnessId),
			);
			return next.length === prev.length ? prev : next;
		});
	}, [activeRoomId, displayedHarnessId]);

	return {
		toasts,
		dismissToast,
		jumpToToast,
	};
}
