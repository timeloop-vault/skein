// The feed-item model — issue #80 D2d. Split out of feedItems.tsx (#460).

import type { HarnessAction } from "./store.ts";

/// A renderable entry in the feed: an action row, a derived turn-boundary
/// separator, a per-turn cost hair-line, or backfill-boundary chrome.
export type FeedItem =
	| {
			type: "row";
			key: string;
			action: HarnessAction;
			/** Arrived over the live event channel (vs the backfill query).
			 *  Live rows slide in; backfilled rows snap. */
			live: boolean;
	  }
	| {
			type: "separator";
			key: string;
			/** The turn_duration row's timestamp — the turn's *end*. */
			timestampMs: number;
			/** Turn length in ms (turn_duration.duration_ms), if recorded. */
			durationMs: number | undefined;
	  }
	| {
			type: "cost";
			key: string;
			/** Tokens the turn processed: input + output + cache reads/writes. */
			tokens: number;
			/** Turn cost in USD. 0 when the harness doesn't report it (Claude
			 *  has no cost field — backend gap #91). */
			usd: number;
	  }
	| {
			type: "backfill-banner";
			key: string;
			/** Backfilled action count (raw events, same unit as the card
			 *  head's "N events"). */
			count: number;
			/** First/last real timestamp among backfilled actions; 0 when
			 *  none carries a clock. */
			rangeStartMs: number;
			rangeEndMs: number;
	  }
	| { type: "backfill-end"; key: string }
	| {
			type: "burst";
			/** `burst-<first constituent id>` — stable as the burst grows. */
			key: string;
			harnessId: string;
			/** Normalized tool name (edit / write / multiedit). */
			tool: string;
			/** Deepest common ancestor of the constituents' directories,
			 *  "/"-normalized full path ("" when nothing is shared). */
			dir: string;
			/** Display scope — `dir` shortened for the gist, `**`-suffixed
			 *  when constituents span subdirectories. */
			scope: string;
			/** Cumulative additions/deletions; undefined when no constituent
			 *  carried patch_info (e.g. fresh Writes with no diff). */
			adds: number | undefined;
			dels: number | undefined;
			/** First/last constituent timestamps — the burst window. */
			firstTimestampMs: number;
			lastTimestampMs: number;
			/** Constituent provenance (uniform — a fold never crosses the
			 *  backfill boundary, or the chrome splice would lie). */
			live: boolean;
			/** The folded rows, in order, for the expanded view. */
			actions: HarnessAction[];
	  };

export interface FlattenOptions {
	/** Render per-turn cost hair-lines (user pref, off by default). */
	showTurnCosts: boolean;
	/** Action ids that arrived over the live event channel (store.ts). */
	liveIds: ReadonlySet<number>;
}
