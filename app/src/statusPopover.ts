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
import { activityToStatus, harnessActivity } from "./harnessActivity.ts";
import type { HarnessKind } from "./types.ts";

const TARGET_SEL = ".h-chip, .tab-status";
// Rows where a lone status dot describes the same harness as the row's
// chip, so the dot can borrow that chip for its kind (harness tab, feed
// row, status-bar seg). The room tab is excluded: its dot is the room
// *aggregate* and the chip's state comes from the store, not the dot.
const ROW_SEL = ".sk-harness-tab, .lc-row, .sk-statusbar .seg";
const HOVER_DELAY_MS = 90;
const EDGE = 8;

const isKind = (k: string): k is HarnessKind => k in HARNESS_KINDS;

export function attachStatusPopover(): () => void {
	let pop: HTMLDivElement | null = null;
	let timer: number | null = null;

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

	// Resolve the {kind, status} to show for a hovered chip/dot. A chip
	// contributes the kind; a dot the status. The row supplies the other
	// half only when it's unambiguous (exactly one chip / one dot) — so a
	// harness tab pairs both, while a room tab's multi-chip row-2 shows
	// just the kind and the room dot shows just the state.
	const resolve = (el: HTMLElement): { kind: string | null; status: string | null } | null => {
		const isChip = el.classList.contains("h-chip");
		const isDot = el.classList.contains("tab-status");
		if (!isChip && !isDot) return null;
		let kind = isChip ? (el.dataset.kind ?? null) : null;
		let status = isDot ? (el.dataset.status ?? null) : null;
		// A chip knows its harness → read that harness's OWN live state from
		// the store, so a room-tab summary chip shows its real state rather
		// than borrowing the room's aggregate dot (#141).
		if (isChip && el.dataset.harnessId) {
			const a = harnessActivity.get(el.dataset.harnessId);
			if (a) status = activityToStatus(a);
		}
		// A lone status dot borrows its row's chip for the kind (harness
		// tab etc.); skipped for the room dot, which is an aggregate.
		if (isDot && !kind) {
			const chips = el.closest<HTMLElement>(ROW_SEL)?.querySelectorAll<HTMLElement>(".h-chip");
			if (chips?.length === 1) kind = chips[0]?.dataset.kind ?? null;
		}
		return kind || status ? { kind, status } : null;
	};

	const render = (el: HTMLDivElement, c: { kind: string | null; status: string | null }) => {
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
		if (c.status) seg("state", c.status, `pv-${c.status}`);
	};

	const onOver = (e: MouseEvent) => {
		const target = e.target as HTMLElement | null;
		const el = target?.closest<HTMLElement>(TARGET_SEL);
		// Skip while inside a modal/palette — the prototype did the same;
		// those surfaces have their own affordances.
		if (!el || el.closest(".sk-modal, .sk-palette")) return;
		const c = resolve(el);
		if (!c) return;
		if (timer !== null) clearTimeout(timer);
		timer = window.setTimeout(() => {
			const p = ensurePop(el);
			if (!p) return;
			render(p, c);
			const r = el.getBoundingClientRect();
			let left = Math.round(r.left + r.width / 2);
			p.style.left = `${left}px`;
			p.style.top = `${Math.round(r.bottom + EDGE)}px`;
			p.classList.add("show");
			// Clamp horizontally so it never spills off-window (the element
			// is centred on `left` via translateX(-50%)).
			const pr = p.getBoundingClientRect();
			if (pr.right > window.innerWidth - EDGE) {
				left = Math.round(window.innerWidth - EDGE - pr.width / 2);
				p.style.left = `${left}px`;
			}
			if (pr.left < EDGE) {
				p.style.left = `${Math.round(EDGE + pr.width / 2)}px`;
			}
		}, HOVER_DELAY_MS);
	};

	const onOut = (e: MouseEvent) => {
		const target = e.target as HTMLElement | null;
		if (!target?.closest(TARGET_SEL)) return;
		if (timer !== null) clearTimeout(timer);
		pop?.classList.remove("show");
	};

	document.addEventListener("mouseover", onOver);
	document.addEventListener("mouseout", onOut);
	return () => {
		if (timer !== null) clearTimeout(timer);
		document.removeEventListener("mouseover", onOver);
		document.removeEventListener("mouseout", onOut);
		pop?.remove();
		pop = null;
	};
}
