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

import { HARNESS_KINDS } from "./data.tsx";
import { activityToStatus, harnessActivity, statusLabel } from "./harnessActivity.ts";
import { mailPopoverText } from "./mailNudge.ts";
import {
	type Breakdown,
	type BreakdownRoomInput,
	type BreakdownRow,
	type BreakdownSubagent,
	buildBreakdown,
} from "./statusBreakdown.ts";
import { subagents } from "./subagents.ts";
import { truncateTip } from "./tipText.ts";
import type { HarnessKind, Room, Status } from "./types.ts";

// #329: `.tab-mail` (the harness tab's unread-mail marker) is a trigger
// too — it sits inside a `.sk-harness-tab` row without being the row's
// chip or dot, so it needs its own entry point into `resolve()` rather
// than relying on bubbling to reach one.
// #355: `[data-sk-tip]` is the generic trigger — any element carrying it
// shows its raw attribute value verbatim (through `truncateTip`), for
// callers that don't fit the structured {kind, status, …} shape below
// (the review Nudge button, the Actions ▾ menu). It replaces `title=`
// on those specifically, not every native tooltip in the app. A caller
// whose trigger is a DISABLED button must put the attribute on a
// wrapping element instead — Chromium fires no mouse events on a
// disabled form control at all (the HTML spec gives it a used
// `pointer-events: none`), so hover only ever reaches an ancestor.
const TARGET_SEL = ".h-chip, .tab-status, .tab-mail, [data-sk-tip]";
// Rows where a lone status dot describes the same harness as the row's
// chip, so the dot can borrow that chip for its kind (harness tab, feed
// row, status-bar seg). The room tab is excluded: its dot is the room
// *aggregate* and the chip's state comes from the store, not the dot.
// #331: an AGGREGATE dot (room tab, group tab — carries `data-room-ids`,
// see `StatusDot`) never goes through this one-line path at all; it
// gets its own multi-row breakdown popover instead, built fresh from
// `getRooms()` and kept live while shown. A harness-tab/chip dot has no
// `data-room-ids` and is unaffected.
const ROW_SEL = ".sk-harness-tab, .lc-row, .sk-statusbar .seg";
const HOVER_DELAY_MS = 90;
const EDGE = 8;

const isKind = (k: string): k is HarnessKind => k in HARNESS_KINDS;

interface Resolved {
	kind: string | null;
	status: string | null;
	agent: { key: string; value: string } | null;
	/** #86: the tool a `permission` status is blocked on, when the
	 *  adapter could say. Only ever populated via the chip's live
	 *  harnessId lookup — a lone status dot has no harness id to ask. */
	tool: string | null;
	/** #298: the subagent name when the `permission` dialog belongs to
	 *  one rather than the main session. Same lookup restriction as
	 *  `tool`. */
	agentType: string | null;
	/** #277: how many subagents are currently working, for the
	 *  "delegating · N agents" wording. Only ever populated via the
	 *  chip's live harnessId lookup, same restriction as `tool`. */
	workingCount: number;
	/** #329: this harness's current unread-mail count/senders, read off
	 *  the row's chip regardless of which element (chip, dot, or the ✉
	 *  marker itself) was hovered — so the segment shows up no matter
	 *  where on the tab the pointer is. 0/[] when there's none. */
	mailCount: number;
	mailFrom: readonly string[];
}

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
			idSubs.set(id, () => {
				unActivity();
				unSubagents();
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

	// Resolve the {kind, status} to show for a hovered chip/dot. A chip
	// contributes the kind; a dot the status. The row supplies the other
	// half only when it's unambiguous (exactly one chip / one dot) — so a
	// harness tab pairs both, while a room tab's multi-chip row-2 shows
	// just the kind and the room dot shows just the state.
	const resolve = (el: HTMLElement): Resolved | null => {
		const isChip = el.classList.contains("h-chip");
		const isDot = el.classList.contains("tab-status");
		const isMail = el.classList.contains("tab-mail");
		if (!isChip && !isDot && !isMail) return null;
		let kind = isChip ? (el.dataset.kind ?? null) : null;
		let status = isDot ? (el.dataset.status ?? null) : null;
		let tool: string | null = null;
		let agentType: string | null = null;
		let workingCount = 0;
		// #248: the chip carries its harness's agent label, already worded
		// by `agentLabel` — the popover repeats it rather than deciding
		// for itself what an opencode agent can be said to be.
		const chip = isChip
			? el
			: el.closest<HTMLElement>(ROW_SEL)?.querySelector<HTMLElement>(".h-chip[data-agent-key]");
		const agent =
			chip?.dataset.agentKey && chip.dataset.agentValue
				? { key: chip.dataset.agentKey, value: chip.dataset.agentValue }
				: null;
		// A chip knows its harness → read that harness's OWN live state from
		// the store, so a room-tab summary chip shows its real state rather
		// than borrowing the room's aggregate dot (#141). Also picks up
		// `permissionTool` (#86) — only available here, since a lone dot
		// has no harness id to ask. #329: the mail marker borrows the same
		// chip (by id, not by the agent-key-filtered `chip` above, which
		// would miss a Claude harness with no agent), so hovering it shows
		// identical state to hovering the rest of the tab.
		const stateChip = isChip
			? el
			: isMail
				? el.closest<HTMLElement>(ROW_SEL)?.querySelector<HTMLElement>(".h-chip[data-harness-id]")
				: undefined;
		if (stateChip?.dataset.harnessId) {
			const a = harnessActivity.get(stateChip.dataset.harnessId);
			if (a) {
				status = activityToStatus(a);
				tool = a.permissionTool;
				agentType = a.permissionAgentType;
			}
			workingCount = subagents.workingCount(stateChip.dataset.harnessId);
		}
		// A lone status dot (or the mail marker) borrows its row's chip for
		// the kind (harness tab etc.); skipped for the room dot, which is
		// an aggregate.
		if ((isDot || isMail) && !kind) {
			const chips = el.closest<HTMLElement>(ROW_SEL)?.querySelectorAll<HTMLElement>(".h-chip");
			if (chips?.length === 1) kind = chips[0]?.dataset.kind ?? null;
		}
		// #329: unread-mail count/senders, carried on the chip as data
		// attributes (mailStore's own state, but this popover is vanilla
		// DOM with no React access to it) — read for every trigger in the
		// row, so the segment shows whether the chip, dot or ✉ itself was
		// hovered.
		const mailChip = isChip
			? el
			: el.closest<HTMLElement>(ROW_SEL)?.querySelector<HTMLElement>(".h-chip[data-mail-count]");
		const mailCount = mailChip?.dataset.mailCount ? Number(mailChip.dataset.mailCount) : 0;
		let mailFrom: readonly string[] = [];
		if (mailChip?.dataset.mailFrom) {
			try {
				mailFrom = JSON.parse(mailChip.dataset.mailFrom) as string[];
			} catch {
				mailFrom = [];
			}
		}
		return kind || status
			? { kind, status, agent, tool, agentType, workingCount, mailCount, mailFrom }
			: null;
	};

	const render = (el: HTMLDivElement, c: Resolved) => {
		el.classList.remove("sk-pop-breakdown", "sk-pop-tip");
		el.replaceChildren();
		const seg = (label: string, value: string, valueClass?: string) => {
			if (el.childElementCount > 0) {
				const sep = document.createElement("span");
				sep.className = "sep";
				sep.textContent = "·";
				el.appendChild(sep);
			}
			const k = document.createElement("span");
			k.className = "pk";
			k.textContent = label;
			const v = document.createElement("span");
			if (valueClass) v.className = valueClass;
			v.textContent = value;
			el.append(k, document.createTextNode(" "), v);
		};
		if (c.kind && isKind(c.kind)) seg("harness", HARNESS_KINDS[c.kind].name);
		if (c.agent) seg(c.agent.key, c.agent.value);
		// #86: "permission needed" (+ tool) rather than the bare word —
		// the dataset value stays the raw Status for the `pv-*` class,
		// only the printed text goes through `statusLabel`. #277:
		// "delegating · N agents" in place of bare "running" likewise.
		if (c.status)
			seg(
				"state",
				statusLabel(c.status as Status, c.tool, c.agentType, c.workingCount),
				`pv-${c.status}`,
			);
		// #329: the tab's unread-mail marker, as a segment rather than its
		// own native tooltip — same one-line style as everything else here.
		if (c.mailCount > 0) seg("mail", mailPopoverText(c.mailCount, c.mailFrom));
	};

	// #355: the generic `[data-sk-tip]` trigger — verbatim text, capped
	// to a sensible number of lines so a page-long override body doesn't
	// produce a screen-sized popover. `white-space: pre-wrap` (the
	// `sk-pop-tip` class) is what makes the preserved `\n`s show.
	const renderTip = (el: HTMLDivElement, text: string) => {
		el.classList.remove("sk-pop-breakdown");
		el.classList.add("sk-pop-tip");
		el.replaceChildren();
		el.textContent = truncateTip(text);
	};

	// ── #331: aggregate breakdown popover ───────────────────────────

	const rule = (): HTMLDivElement => {
		const r = document.createElement("div");
		r.className = "bd-rule";
		return r;
	};

	const renderRow = (row: BreakdownRow, single: boolean): HTMLDivElement => {
		const div = document.createElement("div");
		div.className = "bd-row";
		const dot = document.createElement("span");
		dot.className = `bd-dot pv-${row.status}`;
		dot.textContent = "●";
		const path = document.createElement("span");
		path.className = "bd-path";
		path.textContent = single ? row.harnessName : `${row.roomName} › ${row.harnessName}`;
		const chip = document.createElement("span");
		const kindMeta = HARNESS_KINDS[row.kind];
		chip.className = `bd-chip ${kindMeta.chip}`;
		chip.textContent = kindMeta.label;
		const label = document.createElement("span");
		label.className = `bd-label pv-${row.status}`;
		label.textContent = row.label;
		div.append(dot, path, chip, label);
		return div;
	};

	const renderSubagentLine = (sa: BreakdownSubagent): HTMLDivElement => {
		const div = document.createElement("div");
		div.className = sa.preRestart ? "bd-sub bd-sub-pre" : "bd-sub";
		const type = sa.agentType ?? "agent";
		const desc = sa.description ? ` "${sa.description}"` : "";
		const suffix = sa.preRestart ? " (pre-restart)" : "";
		div.textContent = `↳ ${type}${desc}${suffix}`;
		return div;
	};

	const quietFooterText = (b: Breakdown, single: boolean): string => {
		const parts = b.quiet.map((q) => {
			const kinds = q.kinds.map((k) => HARNESS_KINDS[k].label).join(", ");
			return single ? kinds : `${q.roomName}: ${kinds}`;
		});
		return `+ ${b.quietCount} idle  (${parts.join(" · ")})`;
	};

	const renderBreakdown = (el: HTMLDivElement, aggName: string, b: Breakdown, single: boolean) => {
		el.classList.remove("sk-pop-tip");
		el.classList.add("sk-pop-breakdown");
		el.replaceChildren();

		const header = document.createElement("div");
		header.className = "bd-header";
		const nameSpan = document.createElement("span");
		nameSpan.className = "bd-name";
		nameSpan.textContent = aggName;
		const sep = document.createElement("span");
		sep.className = "sep";
		sep.textContent = "·";
		const statusSpan = document.createElement("span");
		statusSpan.className = `pv-${b.status}`;
		statusSpan.textContent = statusLabel(b.status);
		header.append(nameSpan, sep, statusSpan);
		el.appendChild(header);

		if (b.rows.length > 0) {
			el.appendChild(rule());
			for (const row of b.rows) {
				el.appendChild(renderRow(row, single));
				for (const sa of row.subagents) el.appendChild(renderSubagentLine(sa));
				if (row.hiddenSubagents > 0) {
					const more = document.createElement("div");
					more.className = "bd-sub bd-sub-more";
					more.textContent = `↳ + ${row.hiddenSubagents} more`;
					el.appendChild(more);
				}
			}
			if (b.moreRows > 0) {
				const more = document.createElement("div");
				more.className = "bd-more";
				more.textContent = `+ ${b.moreRows} more`;
				el.appendChild(more);
			}
		}

		if (b.quietCount > 0) {
			el.appendChild(rule());
			const footer = document.createElement("div");
			footer.className = "bd-footer";
			footer.textContent = quietFooterText(b, single);
			el.appendChild(footer);
		}
	};

	// Resolve this dot's room ids against the latest `getRooms()`
	// snapshot and build the breakdown payload. Re-run on every show
	// AND on every live-store emit while shown (see `startBreakdown`) —
	// `getRooms()` itself is a live ref read, so a room closed/renamed
	// mid-hover is picked up too.
	const buildFor = (
		roomIds: readonly string[],
	): { breakdown: Breakdown; rooms: readonly BreakdownRoomInput[] } => {
		const byId = new Map(getRooms().map((r) => [r.id, r]));
		const rooms: BreakdownRoomInput[] = [];
		for (const id of roomIds) {
			const r = byId.get(id);
			if (!r) continue;
			rooms.push({
				id: r.id,
				name: r.name,
				harnesses: r.harnesses.map((h) => ({
					id: h.id,
					kind: h.kind,
					name: h.name,
					pendingNotifications: h.pendingNotifications,
				})),
			});
		}
		const breakdown = buildBreakdown(rooms, {
			activity: harnessActivity.get,
			subagents: subagents.live,
			workingCount: subagents.workingCount,
		});
		return { breakdown, rooms };
	};

	const startBreakdown = (p: HTMLDivElement, el: HTMLElement) => {
		const roomIds = (el.dataset.roomIds ?? "").split(" ").filter((s) => s.length > 0);
		const aggName = el.dataset.aggName ?? "";

		// biome-ignore lint/style/useConst: rebuild and scheduleRebuild are mutually recursive — rebuild re-diffs subscriptions via a scheduler that itself calls rebuild — so both are declared with `let` before either body runs.
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
			const { breakdown, rooms } = buildFor(roomIds);
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
		// #355: the generic trigger — checked first since a `[data-sk-tip]`
		// element carries none of `resolve()`'s own classes and would just
		// fail it.
		const tipText = el.dataset.skTip || undefined;
		// #331: an aggregate dot (room tab, group tab) carries
		// `data-room-ids` and takes the breakdown path entirely, skipping
		// `resolve()` — that function's `isDot` branch would otherwise
		// happily return a bare one-line {status} for it, same as before
		// this feature.
		const isBreakdown =
			tipText === undefined &&
			el.classList.contains("tab-status") &&
			el.dataset.roomIds !== undefined;
		const c = tipText !== undefined || isBreakdown ? null : resolve(el);
		if (tipText === undefined && !isBreakdown && !c) {
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
			if (tipText !== undefined) {
				renderTip(p, tipText);
				positionPopover(p, el);
			} else if (isBreakdown) {
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
