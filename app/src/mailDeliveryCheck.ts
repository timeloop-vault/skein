// `useMailDelivery`'s per-harness `check` (#329): settle any pending nudge
// (#388), read the live unread count, gate, and either nudge through the
// #238 seam or record why not. Plain functions over the hook's ref bag
// (`mailDeliveryState.ts`); the hook serializes calls per harness.

import { invoke } from "@tauri-apps/api/core";
import { HARNESS_KINDS } from "./data.tsx";
import { logBoth } from "./frontendLog.ts";
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import type { GateResult } from "./harnessInput.ts";
import { canSendPrompt, harnessInput, sendPrompt } from "./harnessInput.ts";
import {
	clearSettlement,
	logMailRefusal,
	type MailRefs,
	type MailUnread,
	pendingResult,
} from "./mailDeliveryState.ts";
import { mailHold } from "./mailHold.ts";
import {
	automaticGate,
	decideMailNudge,
	mailHeld,
	mailNudgeText,
	releaseRefusalReason,
} from "./mailNudge.ts";
import { evaluateSettlement, NUDGE_SETTLE_MS } from "./mailSettle.ts";
import { mailStore } from "./mailStore.ts";
import { noteMailState } from "./supervisor/mailFeed.ts";

// #388: settle a still-pending nudge BEFORE asking for the live
// unread count — a rollback here has to land before
// `decideMailNudge` sees this run's count, or it would compare
// against the (wrong) post-nudge `lastNudged` instead of the
// restored pre-nudge one. Returns whether a nudge is still settling —
// folded into the gate passed to `decideMailNudge` so a second
// automatic nudge can't go out while the first one's submit is
// still unproven. `seamSubmit` clears the composer draft
// optimistically the moment `sendPrompt` writes the "\r", so the
// draft alone can't hold this open the way it holds a manually
// typed one — without this flag, mail arriving inside the
// settlement window would paste a second copy on top of a first
// nudge that may not have been submitted yet.
//
// Residual: if the rollback deadline passes with the first
// nudge's Enter dropped through all of #380's own retries, the
// text can still be sitting in the composer while the draft
// reads clean — the re-paste this flag now allows would then
// stack a second copy on top of it. Accepted: the field case
// #388 exists for is a swallowed paste that never reached the
// terminal at all (an empty composer), not a delivered paste
// whose trailing Enter alone was lost after every retry.
function settlePending(refs: MailRefs, harnessId: string): boolean {
	let settlementPending = false;
	const settlement = refs.settleRef.current.get(harnessId);
	if (settlement) {
		const activity = harnessActivity.get(harnessId);
		const decision = evaluateSettlement({
			nowMs: Date.now(),
			settlement,
			activity:
				activity === null
					? null
					: {
							authoritative: activity.authoritative,
							adapterSilent: activity.adapterSilent,
							phase: activity.phase,
							delegationDeferredAt: activity.delegationDeferredAt,
						},
			inputCountNow: harnessInput.userInputCount(harnessId),
		});
		settlementPending = decision.settlementPending;
		if (decision.outcome === "confirmed") {
			clearSettlement(refs, harnessId);
			logBoth(
				"info",
				"skein::mail",
				`[skein] useMailDelivery: nudge confirmed delivered for harness ${harnessId} (#388)`,
			);
		} else if (decision.outcome === "rollback") {
			// `restoreLastNudged` is always set on a rollback outcome —
			// `evaluateSettlement` never returns `null` alongside it.
			if (decision.restoreLastNudged !== null) {
				refs.lastNudgedRef.current.set(harnessId, decision.restoreLastNudged);
			}
			clearSettlement(refs, harnessId);
			logBoth(
				"info",
				"skein::mail",
				`[skein] useMailDelivery: rolling back nudge for harness ${harnessId} — no proof the submit landed (#388)`,
			);
			// #404: this same run's rollback may itself be why a #259
			// adapter-silent degrade fired — the nudge that armed the
			// watchdog never landed, so the tail was never given a real
			// prompt to answer. Recover it before the gate below runs,
			// but only when no human has driven the harness since the
			// nudge was pasted.
			if (decision.attemptRecovery) {
				if (harnessActivity.recoverUnheardSilence(harnessId)) {
					logBoth(
						"info",
						"skein::mail",
						`[skein] useMailDelivery: recovered harness ${harnessId} from a lost first paste before retrying (#404)`,
					);
				} else {
					logBoth(
						"info",
						"skein::mail",
						`[skein] useMailDelivery: declined to recover harness ${harnessId} from a lost first paste (#404)`,
					);
				}
			}
		}
		// "wait": leave the settlement armed and fall through to the
		// normal check — its own timer, or the next event
		// trigger, will re-run this — but refuse a second automatic
		// nudge until it resolves (`settlementPending`, set above).
	}
	return settlementPending;
}

/// Returns whether mail is still pending for `harnessId` after this
/// run (`mailPending`, #386) — `false` only when there's definitely
/// nothing left to retry for (no meta at all); an invoke failure or a
/// missing activity record reads as still-pending `true`, so the
/// bounded retry keeps trying within its window rather than giving up
/// on what may be a transient gap.
/// #413: `releasedByUser` is the tab's "deliver now" — the same attempt,
/// with only the composer-draft guard skipped.
/// `rerun` is the hook's `runSerialized(id, "event")`, fired by the #388
/// settlement timer.
export async function checkMail(
	refs: MailRefs,
	rerun: (harnessId: string) => void,
	harnessId: string,
	releasedByUser = false,
): Promise<boolean> {
	const meta = refs.metaRef.current.get(harnessId);
	if (!meta) return false;
	const settlementPending = settlePending(refs, harnessId);
	let res: MailUnread;
	try {
		res = await invoke<MailUnread>("mail_unread", {
			roomId: meta.roomId,
			harnessId,
		});
	} catch (err: unknown) {
		logBoth("warn", "skein::mail", `[skein] mail_unread failed for ${harnessId}: ${String(err)}`);
		return true;
	}
	mailStore.set(harnessId, res.count, res.fromRoomNames);
	noteMailState(harnessId, { unread: res.count });
	const activity = harnessActivity.get(harnessId);
	if (!activity) return true;
	const body = mailNudgeText(res.count, res.fromRoomNames);
	// #388: a settlement still pending refuses outright, before even
	// consulting `canSendPrompt`/the draft — see the comment on
	// `settlePending` above.
	// #413: an automatic send first lets a confident empty-screen read
	// release an `unknown` draft latch (never on the releasedByUser path,
	// which skips the draft guard anyway).
	if (!settlementPending && !releasedByUser) harnessInput.checkScreen(harnessId);
	const gate: GateResult = settlementPending
		? { ok: false, reason: "a previous nudge is still settling (#388)" }
		: releasedByUser
			? canSendPrompt({
					capabilities: HARNESS_KINDS[meta.kind].capabilities,
					activity,
					registered: harnessInput.isRegistered(harnessId),
					bracketedPasteOn: harnessInput.bracketedPaste(harnessId),
					body,
				})
			: automaticGate(
					canSendPrompt({
						capabilities: HARNESS_KINDS[meta.kind].capabilities,
						activity,
						registered: harnessInput.isRegistered(harnessId),
						bracketedPasteOn: harnessInput.bracketedPaste(harnessId),
						body,
					}),
					harnessInput.draft(harnessId),
				);
	const lastNudged = refs.lastNudgedRef.current.get(harnessId) ?? 0;
	const atStoppingPoint = atSafeStoppingPoint(activity);
	const decision = decideMailNudge({
		atStoppingPoint,
		unread: res.count,
		lastNudged,
		gate,
	});
	// #413: a refused deliver-now — remember why and log it.
	const noteReleaseRefused = (reason: string): void => {
		mailHold.setReleaseRefusal(harnessId, reason);
		logBoth(
			"info",
			"skein::mail",
			`[skein] useMailDelivery: harness ${harnessId} deliver-now refused — ${reason} (#413)`,
		);
	};
	if (!decision.nudge) {
		refs.lastNudgedRef.current.set(harnessId, decision.lastNudged);
		// A refused deliver-now leaves `held` as it was — a non-draft
		// refusal would otherwise read "not held" and drop the entry, and
		// with it the refusal the tab shows.
		if (!releasedByUser) {
			mailHold.set(
				harnessId,
				mailHeld({ unread: res.count, lastNudged: decision.lastNudged, refusal: gate }),
			);
		}
		if (releasedByUser) {
			noteReleaseRefused(
				releaseRefusalReason({
					atStoppingPoint,
					unread: res.count,
					lastNudged: decision.lastNudged,
					gate,
				}),
			);
		}
		// #404: only worth a log line when there's mail actually
		// outstanding AND the gate itself is why nothing went out —
		// "not at a stopping point yet" or "already nudged for this
		// count" aren't refusals, just not-yet.
		const pending = pendingResult(refs, harnessId, res.count, decision.lastNudged);
		if (!gate.ok && pending) {
			logMailRefusal(refs, harnessId, gate);
		}
		return pending;
	}
	logBoth(
		"info",
		"skein::mail",
		`[skein] useMailDelivery: nudging harness ${harnessId} — unread=${res.count} lastNudged=${lastNudged} (#404)`,
	);
	// `sendPrompt` re-checks the gate at call time — a passing
	// `canSendPrompt` above can still lose a race to a phase flip
	// between the two. Only count it as nudged (even provisionally)
	// when the send actually went out; otherwise keep the pre-nudge
	// value (the reset a `nudge: false` decision would have produced)
	// so the next arrival or waiting transition tries again.
	//
	// #388: `result.ok` only means the gate passed and the body was
	// PASTED — the submit "\r" follows `SUBMIT_GAP_MS` later, and can
	// still be silently skipped (the user typed, the phase moved, the
	// terminal respawned; see harnessInput.ts's #380 gap-then-retry).
	// So a passing send doesn't finalize `lastNudged` here — it records
	// it PROVISIONALLY (so no second nudge goes out for this same
	// count while proof is pending) and arms a settlement
	// (`mailSettle.ts`) that either drops it once the submit is
	// proven to have landed, or rolls `lastNudged` back to its
	// pre-nudge value once `NUDGE_SETTLE_MS` passes with no proof —
	// see this settlement's own evaluation in `settlePending`.
	const result = sendPrompt(harnessId, meta.kind, body, {
		automatic: true,
		...(releasedByUser ? { releasedByUser: true } : {}),
	});
	if (!result.ok) {
		refs.lastNudgedRef.current.set(harnessId, lastNudged);
		logMailRefusal(refs, harnessId, result);
		if (!releasedByUser) {
			mailHold.set(harnessId, mailHeld({ unread: res.count, lastNudged, refusal: result }));
		}
		if (releasedByUser) noteReleaseRefused(result.reason);
		return pendingResult(refs, harnessId, res.count, lastNudged);
	}
	mailHold.set(harnessId, false);
	if (releasedByUser) {
		logBoth(
			"info",
			"skein::mail",
			`[skein] useMailDelivery: harness ${harnessId} deliver-now sent — unread=${res.count} (#413)`,
		);
	}
	refs.lastMailRefusalReasonRef.current.delete(harnessId);
	noteMailState(harnessId, { lastRefusal: null, lastNudgeAt: Date.now() });
	refs.lastNudgedRef.current.set(harnessId, decision.lastNudged);
	const prior = refs.settleRef.current.get(harnessId);
	if (prior) clearTimeout(prior.timer);
	const preNudge = prior !== undefined ? Math.min(prior.preNudge, lastNudged) : lastNudged;
	const timer = setTimeout(() => {
		rerun(harnessId);
	}, NUDGE_SETTLE_MS);
	refs.settleRef.current.set(harnessId, {
		preNudge,
		sentAtMs: Date.now(),
		deferredAtSend: activity.delegationDeferredAt,
		turnStarted: false,
		inputCountAtSend: harnessInput.userInputCount(harnessId),
		timer,
	});
	return pendingResult(refs, harnessId, res.count, decision.lastNudged);
}
