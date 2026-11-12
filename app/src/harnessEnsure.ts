// The terminal's one start call (#731): `harness_ensure_running` makes the
// daemon spawn the harness from its stored RECORD (or reuse the process it
// already runs) and attaches this view's channel to it. The view passes the
// cmd it mounted with only as `expectCmd`, an expectation the daemon refuses
// with `stale_record` when the record differs.

import { type Channel, invoke } from "@tauri-apps/api/core";
import { commitMount } from "./harnessMounts.ts";
import { drainRoomsBounded } from "./roomsFlush.ts";
import type { OpencodeVersion } from "./types.ts";

export type PtyEvent = { kind: "data"; chunk: string } | { kind: "exit"; code: number | null };

/** `harness_ensure_running`'s resolved value. */
export interface EnsureResult {
	/** false = a process for this harness already existed; the view only attached. */
	started: boolean;
	id: string;
	/** Whether #215's config injection was non-empty for this spawn. */
	injected: boolean;
	/** #625: a Hermes harness whose status hooks are installed and approved. */
	hermesHooks: boolean;
	/** `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` is truthy in the child's env (#401). */
	mouseClicksDisabled: boolean;
	/** #615: the env profile actually applied (`null` = none). */
	envProfile: string | null;
	sidecarGeneration: number | null;
	opencodeVersion: OpencodeVersion | null;
	sessionId: string | null;
	/** The process was already dead: its `exit` event comes through the channel. */
	exited: boolean;
	exitCode?: number | null;
	/** Names this attachment, so a late detach cannot blank a newer mount. */
	attachId?: number | null;
}

export interface EnsureArgs {
	roomId: string;
	harnessId: string;
	rows: number;
	cols: number;
	/** #615: the harness's override; undefined = follow the global one. */
	envProfile: string | undefined;
	restart: boolean;
	/** Recorded in `harnessMounts` once the daemon has acted on the call. */
	mountKey: string;
	expectCmd: string[];
	onEvent: Channel<PtyEvent>;
}

/** The view went away. Never kills; with an `attachId` the daemon clears the
 *  output sink only if that attachment is still the current one. */
export function detachHarness(harnessId: string, attachId: number | null | undefined): void {
	invoke("harness_detach", { harnessId, attachId: attachId ?? null }).catch((err: unknown) => {
		console.warn(`[skein] harness_detach failed for ${harnessId}:`, errorMessage(err));
	});
}

export interface EnsureDeps {
	drain: () => Promise<void>;
	call: (args: Record<string, unknown>) => Promise<EnsureResult>;
	sleep: (ms: number) => Promise<void>;
	isCancelled: () => boolean;
}

/** Total attempts: the record usually reaches the daemon on the first drain;
 *  the retries cover a rooms sync that failed once and retries on its timer. */
export const ENSURE_ATTEMPTS = 4;
export const ENSURE_BACKOFF_MS = 150;

const RETRYABLE = /^(stale_record|unknown_harness)\b/;

export const errorMessage = (err: unknown): string =>
	err instanceof Error ? err.message : String(err);

export const defaultEnsureDeps = (isCancelled: () => boolean): EnsureDeps => ({
	drain: () => drainRoomsBounded(),
	call: (args) => invoke<EnsureResult>("harness_ensure_running", args),
	sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
	isCancelled,
});

/** Drains the client's pending room mutations (so the record is at the
 *  daemon), then ensures the harness runs. On `stale_record` / `unknown_harness`
 *  drains again and retries a bounded number of times. Returns null when
 *  `isCancelled` turned true during any await (nothing is left attached unless
 *  a call had already succeeded, which is returned instead). Throws the
 *  daemon's `"<code>: <sentence>"` string as an Error otherwise. */
export async function ensureRunning(
	a: EnsureArgs,
	deps: EnsureDeps = defaultEnsureDeps(() => false),
): Promise<EnsureResult | null> {
	let lastErr: unknown;
	for (let attempt = 1; attempt <= ENSURE_ATTEMPTS; attempt++) {
		await deps.drain();
		if (deps.isCancelled()) return null;
		// The key is consumed only once the daemon has acted on the call: a
		// success, or `spawn_failed` (it stops the old process before
		// spawning). Gate refusals happen before anything is stopped, so the
		// key stays uncommitted and the next mount still asks for a restart.
		// Committing happens here even if the effect was cancelled mid-call.
		try {
			const result = await deps.call({
				roomId: a.roomId,
				harnessId: a.harnessId,
				rows: a.rows,
				cols: a.cols,
				envProfile: a.envProfile ?? null,
				restart: a.restart,
				expectCmd: a.expectCmd,
				onEvent: a.onEvent,
			});
			commitMount(a.harnessId, a.mountKey);
			return result;
		} catch (err) {
			lastErr = err;
			if (/^spawn_failed\b/.test(errorMessage(err))) commitMount(a.harnessId, a.mountKey);
			if (!RETRYABLE.test(errorMessage(err)) || attempt === ENSURE_ATTEMPTS) break;
			await deps.sleep(ENSURE_BACKOFF_MS * attempt);
			if (deps.isCancelled()) return null;
		}
	}
	throw lastErr instanceof Error ? lastErr : new Error(String(lastErr));
}
