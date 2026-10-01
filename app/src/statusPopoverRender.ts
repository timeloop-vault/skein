// The one-line popover's content (#132): `harness · agent · state · tasks
// · mail` segments. Split out of statusPopover.ts (#460).

import { HARNESS_KINDS } from "./data.tsx";
import { statusLabel } from "./harnessActivity.ts";
import { mailPopoverText } from "./mailNudge.ts";
import type { Resolved } from "./statusPopoverResolve.ts";
import type { HarnessKind, Status } from "./types.ts";

const isKind = (k: string): k is HarnessKind => k in HARNESS_KINDS;

export const render = (el: HTMLDivElement, c: Resolved) => {
	el.classList.remove("sk-pop-breakdown");
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
			statusLabel(c.status as Status, c.tool, c.agentType, c.workingCount, c.tasks.workingCount),
			`pv-${c.status}`,
		);
	if (c.tasks.text) seg("tasks", c.tasks.text);
	// #329: the tab's unread-mail marker, as a segment rather than its
	// own native tooltip — same one-line style as everything else here.
	if (c.mailCount > 0) seg("mail", mailPopoverText(c.mailCount, c.mailFrom));
};
