// harnessInputSend — the paste/submit mechanics of the #238 nudge seam.
//
// `submit` is a *separate* write from `paste` — the seam pastes the
// body (through xterm's `term.paste()`, so a multi-line body arrives as
// one bracketed-paste message rather than as keystrokes) and then
// writes a bare "\r", exactly the two actions a human would perform.
//
// #380: that submit is no longer written in the same tick as the
// paste. Claude Code 2.1.283 can drop the Enter ending a *first*
// bracketed paste into a freshly spawned harness (startup timing;
// upstream anthropics/claude-code#91205) — `sendPrompt` now pastes,
// waits `SUBMIT_GAP_MS`, then submits, skipping the submit outright
// (logged, text left unsent) if the target, the user, or the phase
// changed in that gap. For kinds that opt in
// (`capabilities.submitRetry`, Claude only), it then follows up with up
// to three more retries on `SUBMIT_RETRY_SCHEDULE_MS`, all inside the
// #259 watchdog's own `ADAPTER_SILENT_AFTER_MS` window — an unverified
// report says Claude may sit on a dropped Enter for longer than one
// quick recheck covers, but nothing past that window is safe to retry
// into (see `submitRetry.ts`'s header for why). See `submitRetry.ts`
// for the pure policy underneath both waits.
//
// `sendPrompt` re-evaluates the gate at call time rather than trusting
// a value a caller rendered a moment earlier — the phase can flip
// between a render and the click that follows it. It also arms the
// #259 silent-adapter watchdog (#363) the same way a typed Enter does
// — this seam's submit never reaches xterm's `onKey`, so without it a
// harness prompted only through here (create_room, a mail nudge) would
// never fall back to the idle heuristic if its adapter stayed silent.

import { checkDraft } from "./composerDraft.ts";
import { HARNESS_KINDS } from "./data.tsx";
import { logBoth } from "./frontendLog.ts";
import { atSafeStoppingPoint, harnessActivity } from "./harnessActivity.ts";
import { canInsertText, canSendPrompt } from "./harnessInputGate.ts";
import type { GateResult } from "./harnessInputGate.ts";
import { harnessInput } from "./harnessInputRegistry.ts";
import type { HarnessInputTarget } from "./harnessInputRegistry.ts";
import { SUBMIT_GAP_MS, SUBMIT_RETRY_SCHEDULE_MS, decideSubmitRetry } from "./submitRetry.ts";
import type { HarnessKind } from "./types.ts";

/// Paste `body` into `harnessId`'s terminal and submit it, after
/// re-checking `canSendPrompt` against the *current* state — never the
/// state a caller rendered a moment ago. Returns the gate result either
/// way, so a caller that raced a phase change can show why it refused.
///
/// #380: the submit doesn't land in the same tick as the paste — see
/// this file's header comment and `submitRetry.ts`. The paste and the
/// gate result are still synchronous; only the "\r" (and, for a
/// retry-capable kind, its possible retry) happen later, off a timer.
///
/// #383: `opts.automatic` folds `checkDraft` on top of `canSendPrompt`
/// — set only by an automatic sender (a mail nudge); a manual send
/// (the Nudge button, a prompt-library item) passes no `opts` and must
/// not consult the draft at all, since a human choosing to send right
/// now is not something a composer-state guess should override.
export function sendPrompt(
	harnessId: string,
	kind: HarnessKind,
	body: string,
	opts?: { automatic?: boolean; releasedByUser?: boolean },
): GateResult {
	const target = harnessInput.target(harnessId);
	const gate = canSendPrompt({
		capabilities: HARNESS_KINDS[kind].capabilities,
		activity: harnessActivity.get(harnessId),
		registered: target !== undefined,
		bracketedPasteOn: target?.bracketedPaste() ?? false,
		body,
	});
	if (!gate.ok) {
		logBoth(
			"info",
			"skein::seam",
			`[skein] sendPrompt: harness ${harnessId} refused — ${gate.reason} (#404)`,
		);
		return gate;
	}
	// #413: `releasedByUser` (the tab's "deliver now") skips ONLY this
	// draft recheck; the gate above and everything after still apply.
	if (opts?.automatic && !opts.releasedByUser) {
		const draftRefusal = checkDraft(harnessInput.draft(harnessId));
		if (draftRefusal) {
			logBoth(
				"info",
				"skein::seam",
				`[skein] sendPrompt: harness ${harnessId} automatic send refused — ${draftRefusal.reason} (#404)`,
			);
			return draftRefusal;
		}
	}
	// A passing gate required `registered: target !== undefined`, so
	// `target` is set here by construction.
	const t = target as HarnessInputTarget;
	t.paste(body);
	logBoth(
		"info",
		"skein::seam",
		`[skein] sendPrompt: pasted ${body.length} char(s) into harness ${harnessId} (${
			opts?.automatic ? "automatic" : "manual"
		}) (#404)`,
	);
	harnessInput.noteDraftEvent(harnessId, { type: "seamPaste" });

	// #381: snapshot which stopping point the paste landed at — a plain
	// `waiting` (`null`) or a specific armed #277 delegation deferral
	// (that deferral's own `delegationDeferredAt` timestamp). The
	// submit-time recheck below requires the SAME one, not just "still
	// `waiting`": a `waiting` recheck alone would let a submit through
	// after the harness left the deferral it was pasted under, started a
	// fresh turn, and got re-deferred — a different arm than the one this
	// paste is about.
	const deferredAtPaste = harnessActivity.get(harnessId)?.delegationDeferredAt ?? null;

	const inputCountAtPaste = harnessInput.userInputCount(harnessId);
	setTimeout(() => {
		// Skip the submit outright — leaving the pasted text unsent
		// rather than risk it — if anything about the target changed in
		// the gap: a respawn (registration moved on), the user now
		// typing (a machine "\r" landing on top of a human's own input
		// is never safe), or the harness having left the stopping point
		// it was pasted at. That last check mirrors `canSendPrompt`
		// itself rather than singling out `permission` — a blind "\r" is
		// just as wrong after any other stopping-point drift (the turn
		// ended and started a new one, the deferral changed arm, the
		// harness exited) as it is into an open dialog; `permission` only
		// gets its own message because it's the common, nameable case
		// (#86's territory, not this one's).
		if (harnessInput.target(harnessId) !== t) {
			logBoth(
				"warn",
				"skein::seam",
				`[skein] sendPrompt: harness ${harnessId}'s terminal changed before the submit — leaving the paste unsent (#380)`,
			);
			return;
		}
		if (harnessInput.userInputCount(harnessId) !== inputCountAtPaste) {
			logBoth(
				"warn",
				"skein::seam",
				`[skein] sendPrompt: user typed into harness ${harnessId} before the submit — leaving the paste unsent (#380)`,
			);
			return;
		}
		const activityAtSubmit = harnessActivity.get(harnessId);
		const stillAtStoppingPoint =
			activityAtSubmit !== null &&
			atSafeStoppingPoint(activityAtSubmit) &&
			activityAtSubmit.delegationDeferredAt === deferredAtPaste;
		if (!stillAtStoppingPoint) {
			const phaseAtSubmit = activityAtSubmit?.phase;
			const why =
				phaseAtSubmit === "permission"
					? "opened a permission dialog"
					: activityAtSubmit !== null && atSafeStoppingPoint(activityAtSubmit)
						? "moved to a different delegation deferral than the one it was pasted under"
						: `moved to ${phaseAtSubmit ?? "no activity record"}`;
			logBoth(
				"warn",
				"skein::seam",
				`[skein] sendPrompt: harness ${harnessId} ${why} before the submit — leaving the paste unsent (#380, #381)`,
			);
			return;
		}
		t.submit();
		logBoth(
			"info",
			"skein::seam",
			`[skein] sendPrompt: submit written into harness ${harnessId} (#404)`,
		);
		harnessInput.noteDraftEvent(harnessId, { type: "seamSubmit" });
		// #363: this submit never touches xterm's `onKey`, so without this
		// call the #259 silent-adapter watchdog would never arm for a prompt
		// that arrived through this seam — `create_room`'s first prompt, a
		// mail nudge.
		harnessActivity.notePromptSubmitted(harnessId);

		// #380: up to three retries, only for kinds that opt in (Claude)
		// — an unverified report says Claude may sit on a dropped Enter
		// for longer than one quick recheck covers. Every attempt in
		// `SUBMIT_RETRY_SCHEDULE_MS` stays inside the #259 watchdog's
		// `ADAPTER_SILENT_AFTER_MS` window on purpose: past that point
		// `degradeSilentAdapter` has already flipped `authoritative`
		// false and `adapterSilent` true, so "phase left waiting" is no
		// longer authoritative proof of anything — `decideSubmitRetry`'s
		// `watched` check refuses the moment that happens, rather than
		// keep retrying blind.
		if (!HARNESS_KINDS[kind].capabilities.submitRetry) return;
		// One subscription for the whole retry window — see
		// `decideSubmitRetry`'s `leftStoppingPointSinceSend` docs for
		// why a single transition out of `waiting`, anywhere in the
		// window, ends the sequence for every attempt still to come.
		// This only ever fires for the plain-`waiting` arm — see
		// `deferredAtPaste` below for how a delegation-deferred arm's
		// equivalent is caught instead.
		let leftStoppingPointSinceSend = false;
		// Deliberately not tied to this harness's unmount/respawn: each
		// attempt below self-cleans on its own schedule regardless of
		// what happens to the harness in between, and a target that goes
		// away or respawns in the meantime is caught by
		// `decideSubmitRetry`'s own `sameTarget` check (and a harness
		// that fully exits shows up there as `phase: null`) — no extra
		// unsubscribe-on-teardown path needed.
		const unsubscribe = harnessActivity.subscribeTransitions((id, from) => {
			if (id === harnessId && from === "waiting") leftStoppingPointSinceSend = true;
		});
		const attemptTimers: Array<ReturnType<typeof setTimeout>> = [];
		const attempt = (attemptIndex: number) => {
			const isLastAttempt = attemptIndex === SUBMIT_RETRY_SCHEDULE_MS.length - 1;
			const a = harnessActivity.get(harnessId);
			const decision = decideSubmitRetry({
				capable: true,
				watched: a === null ? false : a.authoritative && !a.adapterSilent,
				leftStoppingPointSinceSend,
				phase: a?.phase ?? null,
				// #381: `deferredAtPaste` (`null` for a plain-`waiting` send)
				// is the arm this submit belongs to; `a`'s current
				// `delegationDeferredAt` is compared against it on every
				// attempt — a disarm, a flush to `waiting`, or a fresh
				// re-arm with a different timestamp all read as "changed" and
				// end the sequence, same as `sendPrompt`'s own submit-time
				// recheck above.
				deferredAtSend: deferredAtPaste,
				deferredAtNow: a?.delegationDeferredAt ?? null,
				// Measured from the paste, same snapshot the gap-check
				// above already required to be unchanged by submit time
				// — so this is equally "since the submit" in practice.
				userInputSinceSend: harnessInput.userInputCount(harnessId) !== inputCountAtPaste,
				sameTarget: harnessInput.target(harnessId) === t,
			});
			if (decision.retry) {
				logBoth(
					"info",
					"skein::seam",
					`[skein] sendPrompt: retry ${attemptIndex + 1}/${SUBMIT_RETRY_SCHEDULE_MS.length} into harness ${harnessId} — the first Enter didn't appear to land (#380)`,
				);
				t.submit();
			} else {
				// A refusal ends the whole sequence — cancel whatever
				// hasn't fired yet rather than let a stale reason keep
				// retrying on its own schedule.
				for (let i = attemptIndex + 1; i < attemptTimers.length; i++) {
					const pending = attemptTimers[i];
					if (pending !== undefined) clearTimeout(pending);
				}
			}
			if (isLastAttempt || !decision.retry) unsubscribe();
		};
		for (const [attemptIndex, delay] of SUBMIT_RETRY_SCHEDULE_MS.entries()) {
			attemptTimers.push(setTimeout(() => attempt(attemptIndex), delay));
		}
	}, SUBMIT_GAP_MS);

	return gate;
}

/// Paste `text` into `harnessId`'s terminal — and only that, never
/// `submit` — after re-checking `canInsertText` against the *current*
/// state. #41's drop-a-file caller: a path dropped mid-turn seeds the
/// input for whatever the user types next, it does not send anything on
/// the harness's behalf. Same re-check-at-call-time contract as
/// `sendPrompt`.
export function insertText(harnessId: string, kind: HarnessKind, text: string): GateResult {
	const target = harnessInput.target(harnessId);
	const gate = canInsertText({
		capabilities: HARNESS_KINDS[kind].capabilities,
		activity: harnessActivity.get(harnessId),
		registered: target !== undefined,
	});
	if (!gate.ok) return gate;
	// A passing gate required `registered: target !== undefined`, so
	// `target` is set here by construction.
	target?.paste(text);
	return gate;
}
