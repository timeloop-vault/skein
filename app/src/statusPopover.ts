// #132: one lightweight hover popover for status dots + harness chips,
// replacing the native `title=` (which was slow, unstyled, and could
// only name the kind — not the state).
//
// Event-delegated like the design prototype (skein-controls.js): a
// single popover element + two document listeners, rather than per-dot
// React state. A dot in a row reads the row's chip for the kind, and a
// chip reads the row's dot for the state, so a harness tab shows
// "harness Claude Code · state waiting" from either.
//
// The element is created lazily and appended to the hovered element's
// `.sk-app` ancestor so it inherits the active theme tokens.

import { backgroundTasks } from "./backgroundTasks.ts";
import { harnessActivity } from "./harnessActivity.ts";
import { buildFor, renderBreakdown } from "./statusPopoverBreakdown.ts";
import { render } from "./statusPopoverRender.ts";
import { resolve } from "./statusPopoverResolve.ts";
import { subagents } from "./subagents.ts";
import type { Room } from "./types.ts";
import "./statusPopover.css";

// #329: `.tab-mail` (the harness tab's unread-mail marker) is a trigger
// too — it sits inside a `.sk-harness-tab` row without being the row's
// chip or dot, so it needs its own entry point into `resolve()` rather
// than relying on bubbling to reach one.
const TARGET_SEL = ".h-chip, .tab-status, .tab-mail";
const HOVER_DELAY_MS = 90;
const EDGE = 8;

export function attachStatusPopover(getRooms: () => readonly Room[]): () => void {
	let pop: HTMLDivElement | null = null;
	let timer: number | null = null;
	// #331: live subscriptions held while a breakdown popover is shown —
	// one pair (activity + subagents) per harness id currently resolved,
	// kept in sync with `getRooms()` by `syncIdSubs` on every rebuild (a
	// harness added to/removed from a hovered room mid-hover is picked
	// up, not just its existing members' transitions) — plus a global
	// `subscribeTransitions` listener that catches a transition for an
	// id not yet subscribed (e.g. a brand-new harness) and schedules the
	// rebuild that then subscribes it, and the rAF handle a burst of
	// store emits is coalesced through. All cleared by `hide()`, the one
	// place a shown/pending popover is torn down (#314).
	const idSubs = new Map<string, () => void>();
	let transitionUnsub: (() => void) | null = null;
	let rebuildRaf: number | null = null;

	const clearLiveSubs = () => {
		for (const un of idSubs.values()) un();
		idSubs.clear();
		if (transitionUnsub) {
			transitionUnsub();
			transitionUnsub = null;
		}
		if (rebuildRaf !== null) {
			cancelAnimationFrame(rebuildRaf);
			rebuildRaf = null;
		}
	};

	// Subscribe/unsubscribe per-harness activity + subagent listeners
	// so the held set always matches `ids` — called at the end of every
	// rebuild with the ids just resolved from `getRooms()`.
	const syncIdSubs = (ids: readonly string[], scheduleRebuild: () => void) => {
		const wanted = new Set(ids);
		for (const [id, un] of idSubs) {
			if (!wanted.has(id)) {
				un();
				idSubs.delete(id);
			}
		}
		for (const id of wanted) {
			if (idSubs.has(id)) continue;
			const unActivity = harnessActivity.subscribe(id, scheduleRebuild);
			const unSubagents = subagents.subscribe(id, scheduleRebuild);
			const unTasks = backgroundTasks.subscribe(id, scheduleRebuild);
			idSubs.set(id, () => {
				unActivity();
				unSubagents();
				unTasks();
			});
		}
	};

	const ensurePop = (host: HTMLElement): HTMLDivElement | null => {
		const app = host.closest<HTMLElement>(".sk-app");
		if (!app) return null;
		if (!pop) {
			pop = document.createElement("div");
			pop.className = "sk-pop";
		}
		if (pop.parentElement !== app) app.appendChild(pop);
		return pop;
	};

	// Position below the hovered element, clamped horizontally so it
	// never spills off-window (the element is centred on `left` via
	// translateX(-50%)). Shared by both the one-line and breakdown
	// popovers.
	const positionPopover = (p: HTMLDivElement, el: HTMLElement) => {
		const r = el.getBoundingClientRect();
		let left = Math.round(r.left + r.width / 2);
		p.style.left = `${left}px`;
		p.style.top = `${Math.round(r.bottom + EDGE)}px`;
		p.classList.add("show");
		const pr = p.getBoundingClientRect();
		if (pr.right > window.innerWidth - EDGE) {
			left = Math.round(window.innerWidth - EDGE - pr.width / 2);
			p.style.left = `${left}px`;
		}
		if (pr.left < EDGE) {
			p.style.left = `${Math.round(EDGE + pr.width / 2)}px`;
		}
	};

	const startBreakdown = (p: HTMLDivElement, el: HTMLElement) => {
		const roomIds = (el.dataset.roomIds ?? "").split(" ").filter((s) => s.length > 0);
		const aggName = el.dataset.aggName ?? "";

		let rebuild: () => void;
		const scheduleRebuild = () => {
			if (rebuildRaf !== null) return;
			rebuildRaf = requestAnimationFrame(() => {
				rebuildRaf = null;
				rebuild();
			});
		};
		rebuild = () => {
			// The dot can be unmounted while subscribed — a drag reorder, a
			// group row disappearing when its segment stops being active —
			// with no mouseout to hide it (#314's reasoning, applied to a
			// live re-render rather than just the show path). Positioning
			// against a detached element would otherwise land at (0,0).
			if (!el.isConnected) {
				hide();
				return;
			}
			const { breakdown, rooms } = buildFor(getRooms, roomIds);
			const harnessIds = rooms.flatMap((r) => r.harnesses.map((h) => h.id));
			renderBreakdown(p, aggName, breakdown, rooms.length === 1);
			positionPopover(p, el);
			// #331: re-diff against the CURRENT room membership every
			// rebuild, not just at show time — a harness added to (or
			// removed from) a hovered room must start (or stop) being
			// subscribed too.
			syncIdSubs(harnessIds, scheduleRebuild);
		};

		clearLiveSubs();
		// A transition for an id not yet in `idSubs` means a harness this
		// popover doesn't know about yet just changed phase (most likely:
		// it was just added to a hovered room) — schedule a rebuild so
		// `syncIdSubs` picks it up. Once subscribed directly, its own
		// per-id listener covers it and this is a no-op for that id.
		transitionUnsub = harnessActivity.subscribeTransitions((id) => {
			if (!idSubs.has(id)) scheduleRebuild();
		});
		rebuild();
	};

	// #314: the one place a pending/shown popover gets torn down. Used by
	// both the mouseout path and every early-out in onOver — a removed
	// element never fires mouseout, so those early-outs must hide rather
	// than just returning, or a popover shown for a chip the picker then
	// unmounted is stuck forever. #331: also drops the breakdown's live
	// subscriptions, if any are held.
	const hide = () => {
		if (timer !== null) clearTimeout(timer);
		timer = null;
		clearLiveSubs();
		pop?.classList.remove("show");
	};

	const onOver = (e: MouseEvent) => {
		const target = e.target as HTMLElement | null;
		const el = target?.closest<HTMLElement>(TARGET_SEL);
		// Skip while inside a modal/palette — the prototype did the same;
		// those surfaces have their own affordances. #314: hide rather than
		// leaving a stale popover from a previously-hovered element.
		if (!el || el.closest(".sk-modal, .sk-palette")) {
			hide();
			return;
		}
		// #331: an aggregate dot (room tab, group tab) carries
		// `data-room-ids` and takes the breakdown path entirely, skipping
		// `resolve()` — that function's `isDot` branch would otherwise
		// happily return a bare one-line {status} for it, same as before
		// this feature.
		const isBreakdown = el.classList.contains("tab-status") && el.dataset.roomIds !== undefined;
		const c = isBreakdown ? null : resolve(el);
		if (!isBreakdown && !c) {
			hide();
			return;
		}
		if (timer !== null) clearTimeout(timer);
		timer = window.setTimeout(() => {
			timer = null;
			// #314: the element can be unmounted (e.g. the `+ harness` picker
			// closing on click) within the delay window, with no mouseout to
			// cancel the timer — never show a popover for a detached element.
			if (!el.isConnected) return;
			const p = ensurePop(el);
			if (!p) return;
			if (isBreakdown) {
				startBreakdown(p, el);
			} else if (c) {
				render(p, c);
				positionPopover(p, el);
			}
		}, HOVER_DELAY_MS);
	};

	const onOut = (e: MouseEvent) => {
		const target = e.target as HTMLElement | null;
		if (!target?.closest(TARGET_SEL)) return;
		hide();
	};

	// #314: a click changes the surface under the popover (standard tooltip
	// behaviour) — e.g. the `+ harness` picker unmounting the hovered chip.
	document.addEventListener("pointerdown", hide);
	document.addEventListener("mouseover", onOver);
	document.addEventListener("mouseout", onOut);
	return () => {
		hide();
		document.removeEventListener("pointerdown", hide);
		document.removeEventListener("mouseover", onOver);
		document.removeEventListener("mouseout", onOut);
		pop?.remove();
		pop = null;
	};
}
