// The webview's half of the daemon's prompt delivery (#733). The daemon pastes
// and submits; this hook feeds it the two facts only a client has
// (`clientFacts.ts` -> `harness_client_facts`) and tells the composer-draft
// tracker what the daemon wrote (`skein://delivery-seam`). Mounted once, from
// `useAppBackgroundWiring`. Nothing here writes to a terminal.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { clientFacts, createFactsDiffer, createReportCounter } from "./clientFacts.ts";
import { type DeliverySeamPayload, seamDraftEvent } from "./deliverySeam.ts";
import { harnessInput } from "./harnessInput.ts";
import { createHeldRecheck } from "./heldRecheck.ts";
import { mailHold } from "./mailHold.ts";

/// Fire-and-forget, errors swallowed, like `phaseInput.ts`.
function sendFacts(harnessId: string, facts: ReturnType<typeof clientFacts>, report: number): void {
	try {
		void invoke("harness_client_facts", {
			harnessId,
			draft: facts.draft,
			userInputSeq: facts.userInputSeq,
			report,
		}).catch((e: unknown) => {
			console.debug("[skein] harness_client_facts failed", e);
		});
	} catch (e) {
		console.debug("[skein] harness_client_facts failed", e);
	}
}

export function useDaemonDelivery(): void {
	useEffect(() => {
		const differ = createFactsDiffer();
		const nextReport = createReportCounter();
		// A keystroke bumps the counter and moves the draft in the same tick;
		// one report per harness per tick, forced if any request was.
		const pending = new Map<string, boolean>();
		const report = (id: string, force: boolean): void => {
			const queued = pending.has(id);
			pending.set(id, (pending.get(id) ?? false) || force);
			if (queued) return;
			queueMicrotask(() => {
				const forced = pending.get(id) ?? false;
				pending.delete(id);
				const next = differ.next(
					id,
					clientFacts(harnessInput.draft(id), harnessInput.userInputCount(id)),
					forced,
				);
				if (next !== null) sendFacts(id, next, nextReport());
			});
		};
		const offFacts = harnessInput.subscribeFacts((id) => report(id, false));
		const offRegistered = harnessInput.subscribeRegistered((id) => report(id, true));
		// While mail is held, keep re-reading the screen: it is what releases
		// an `unknown` draft latch, and the daemon cannot read it (#733).
		const recheck = createHeldRecheck({
			check: (id) => {
				harnessInput.checkScreen(id);
			},
			isRegistered: (id) => harnessInput.isRegistered(id),
		});
		const offHeld = mailHold.subscribeHeld((id, held) => recheck.setHeld(id, held));
		const seam = listen<DeliverySeamPayload>("skein://delivery-seam", (event) => {
			const ev = seamDraftEvent(event.payload);
			if (ev !== null) harnessInput.noteDraftEvent(event.payload.harnessId, ev);
		});
		return () => {
			offFacts();
			offRegistered();
			offHeld();
			recheck.dispose();
			void seam.then((un) => un());
		};
	}, []);
}
