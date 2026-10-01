// The per-harness state `useMailDelivery` owns (#329), as one bag of refs
// the hook creates and the plain functions in `mailDeliveryCheck.ts` and
// `mailDeliveryRetry.ts` receive. The refs live in the hook — one
// instance per mount — so nothing here is module-level state; this file
// holds only the shapes and the small helpers that touch those refs.

import type { MutableRefObject } from "react";
import { logBoth } from "./frontendLog.ts";
import { harnessInput } from "./harnessInput.ts";
import type { GateResult } from "./harnessInput.ts";
import { isDraftHold } from "./mailNudge.ts";
import { mailPending } from "./mailRetry.ts";
import { noteMailState } from "./supervisor/mailFeed.ts";
import type { HarnessKind } from "./types.ts";

/// DTO mirror of `agent_api::verbs::MailUnread` — see `mail_unread` in
/// `app/src-tauri/src/agent_api/commands.rs`.
export interface MailUnread {
	count: number;
	fromRoomNames: string[];
}

export interface HarnessMeta {
	roomId: string;
	kind: HarnessKind;
}

/// #386: when this harness's retry window was last (re-)armed, and the
/// pending timer for its next tick, if any.
export interface RetryEntry {
	armedAtMs: number;
	timer: ReturnType<typeof setTimeout> | null;
}

/// #388: settlement state for a nudge that was just pasted, but not yet
/// proven to have been submitted — see `useMailDelivery.ts`'s header and
/// `mailSettle.ts`. `preNudge` is the `lastNudged` value from BEFORE this
/// nudge, restored on rollback; `sentAtMs` is when the paste happened;
/// `deferredAtSend`/`turnStarted` are the same evidence `submitRetry.ts`
/// uses, gathered here instead of there because this settlement outlives
/// any one `sendPrompt` call.
export interface SettleEntry {
	preNudge: number;
	sentAtMs: number;
	deferredAtSend: number | null;
	turnStarted: boolean;
	/// #404: `harnessInput.userInputCount(id)` snapshotted at
	/// paste time — compared again at rollback to tell whether a
	/// human drove the harness in between, which
	/// `shouldRecoverSilence` needs to decide whether a
	/// lost-silence recovery is safe to attempt.
	inputCountAtSend: number;
	timer: ReturnType<typeof setTimeout>;
}

export interface MailRefs {
	// harnessId → {roomId, kind}, rebuilt from `rooms` on every change —
	// a ref so the event/transition listeners (mounted once, empty
	// deps) always see the latest membership without re-subscribing.
	metaRef: MutableRefObject<Map<string, HarnessMeta>>;
	// Which harnesses have already been seeded once this session, so a
	// `rooms` change doesn't re-seed a harness this hook already knows.
	seededRef: MutableRefObject<Set<string>>;
	// Unread count as of the last nudge actually sent, per harness.
	// Empty at mount = restart replay, per mailNudge.ts.
	lastNudgedRef: MutableRefObject<Map<string, number>>;
	// Per-harness promise chain so a mail-changed event and a waiting
	// transition arriving together run `check` one at a time, not
	// concurrently. Resolves to whether mail is still pending after that
	// run, which is what schedules (or stops) the #386 retry.
	inFlightRef: MutableRefObject<Map<string, Promise<boolean>>>;
	retryRef: MutableRefObject<Map<string, RetryEntry>>;
	settleRef: MutableRefObject<Map<string, SettleEntry>>;
	// False once the hook has unmounted, so a `check` still in flight at
	// that moment can't schedule a timer the unmount cleanup already missed.
	mountedRef: MutableRefObject<boolean>;
	// #404: per-harness last-logged gate/sendPrompt refusal reason, so the
	// #386 retry tick (every `MAIL_RETRY_INTERVAL_MS`, 2 s) doesn't spam
	// `skein.log` with the same unchanging reason on every tick — only a
	// CHANGE in reason is worth another line. Cleared once a nudge goes
	// out (the reason it was tracking no longer applies) or the harness
	// is forgotten.
	lastMailRefusalReasonRef: MutableRefObject<Map<string, string>>;
}

export function logMailRefusal(refs: MailRefs, harnessId: string, refusal: GateResult): void {
	const reason = refusal.ok ? "" : refusal.reason;
	if (refs.lastMailRefusalReasonRef.current.get(harnessId) === reason) return;
	refs.lastMailRefusalReasonRef.current.set(harnessId, reason);
	noteMailState(harnessId, { lastRefusal: reason === "" ? null : reason });
	// #413: name what last moved the draft off clean (event class only,
	// never content) when the refusal is the draft guard's.
	const cause = isDraftHold(refusal) ? harnessInput.draftCause(harnessId) : null;
	const causeNote = cause
		? ` [cause: ${cause.event}, ${Math.round((Date.now() - cause.at) / 1000)}s ago]`
		: "";
	logBoth(
		"info",
		"skein::mail",
		`[skein] useMailDelivery: harness ${harnessId} nudge refused — ${reason}${causeNote} (#404, #413)`,
	);
}

// #404: `mailPending`'s own verdict for this run, wrapping the throttle
// clear so a harness that's caught up (no mail left to retry for) drops
// its remembered reason — a LATER refusal for a fresh batch of mail
// logs again even if it happens to be the same reason as last time,
// rather than reading as still-throttled from mail that's long gone.
export function pendingResult(
	refs: MailRefs,
	harnessId: string,
	count: number,
	lastNudged: number,
): boolean {
	const pending = mailPending(count, lastNudged);
	if (!pending) {
		refs.lastMailRefusalReasonRef.current.delete(harnessId);
		noteMailState(harnessId, { lastRefusal: null });
	}
	return pending;
}

export function clearRetry(refs: MailRefs, harnessId: string): void {
	const entry = refs.retryRef.current.get(harnessId);
	if (entry?.timer) clearTimeout(entry.timer);
	refs.retryRef.current.delete(harnessId);
}

export function clearSettlement(refs: MailRefs, harnessId: string): void {
	const entry = refs.settleRef.current.get(harnessId);
	if (entry) clearTimeout(entry.timer);
	refs.settleRef.current.delete(harnessId);
}
