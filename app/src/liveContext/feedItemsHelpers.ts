// Pure helpers behind flattenFeed's burst-fold and per-turn-cost logic.
// Split out of feedItems.tsx (#19) — moved verbatim.

import { type Payload, num, obj, parsePayload, str } from "./payload.ts";
import type { HarnessAction } from "./store.ts";

/// Kinds that never render as their own row and aren't derived chrome
/// either: consumed elsewhere. Keep in sync with the dispatcher in
/// rows.tsx, whose default case returns null for these (plus turn_cost /
/// turn_duration, which this module turns into derived items instead).
/// `cost_state` (#199) is here because it is a session *snapshot*, not
/// an event: it is consumed by the card head via `sessionTotals`, and a
/// row for it would be an invisible item that breaks bursts and inflates
/// the unseen counter.
export const SKIP_KINDS = new Set(["away_summary", "reasoning", "cost_state"]);

/// Burst fold rule: consecutive emitted items from the same harness,
/// same normalized tool, gap < 5 s. Scope renders as the constituents'
/// deepest common ancestor — `skills/**` when they span directories,
/// the exact `…/src/auth/` when they don't. This widens handover §6's
/// "same dirname" letter: the prototype's own bursts span subdirs with
/// `/**` scopes, and dogfooding hit the canonical miss (one file per
/// sibling directory, e.g. seven skills/<name>/SKILL.md edits) on day
/// one. §12 marks the fold rule as a dogfooding knob. "Consecutive" is
/// over *items* — kinds that emit nothing (reasoning, suppressed
/// turn_cost) don't break a streak the user can't see broken.
export const BURST_GAP_MS = 5000;
/// Minimum constituents before folding: a pair isn't a storm, and folding
/// it would hide detail for no real de-noising. Tune per §12.
export const BURST_MIN = 3;

export interface BurstCandidate {
	action: HarnessAction;
	dir: string;
	tool: string;
	adds: number | undefined;
	dels: number | undefined;
	live: boolean;
}

/// Path without its last segment ("" for bare filenames). Handles both
/// separators — harnesses emit native paths, and a "/"-only split would
/// flatten every Windows path to "", degrading burst scopes.
export function dirname(path: string): string {
	const trimmed = path.replace(/[\\/]+$/, "");
	const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
	return i === -1 ? "" : trimmed.slice(0, i);
}

/// Shorten an (often absolute) directory for the burst gist: the last
/// two segments with a trailing slash, "…/"-prefixed when deeper.
export function shortScope(dir: string): string {
	if (!dir) return "./";
	const segs = dir.split(/[\\/]/).filter(Boolean);
	const tail = segs.slice(-2).join("/");
	return `${segs.length > 2 ? "…/" : ""}${tail}/`;
}

/// Deepest directory containing every input, in "/"-normalized form
/// (Windows backslashes normalize so mixed-separator runs still share
/// ancestry). "" when nothing is shared.
export function commonAncestor(dirs: string[]): string {
	const norm = dirs.map((d) => d.replace(/\\/g, "/"));
	let anc = norm[0] ?? "";
	for (const d of norm) {
		while (anc && d !== anc && !d.startsWith(`${anc}/`)) {
			const i = anc.lastIndexOf("/");
			anc = i === -1 ? "" : anc.slice(0, i);
		}
		if (!anc) break;
	}
	return anc;
}

/// A patch row that may fold into a burst: a successful edit/write with
/// a file and a real timestamp. Excluded on purpose: errored patches
/// (they render as error rows and should visibly break a storm) and the
/// opencode `{files, hash}` snapshot flavor (no tool, no deltas, ts=0).
export function burstCandidate(a: HarnessAction, live: boolean): BurstCandidate | undefined {
	if (a.timestampMs <= 0) return undefined;
	const p: Payload = parsePayload(a.payload);
	if (p.is_error === true) return undefined;
	const raw = (str(p.tool) ?? "").toLowerCase();
	if (raw !== "edit" && raw !== "write" && raw !== "multiedit") return undefined;
	const files = Array.isArray(p.files) ? p.files : [];
	const file = str(files[0]);
	if (!file) return undefined;
	const pi = obj(p.patch_info);
	return {
		action: a,
		dir: dirname(file),
		// multiedit folds (and labels) as "write" — EditRow renders it as
		// write, and a fold key the user can't see would split visually
		// identical rows.
		tool: raw === "multiedit" ? "write" : raw,
		adds: pi ? num(pi.additions) : undefined,
		dels: pi ? num(pi.deletions) : undefined,
		live,
	};
}

/// The two turn_cost payload shapes (see docs/live-context-d2-buildmap.md
/// "Claude ↔ opencode payload divergence"):
///
/// - Claude — `{model, request_id, stop_reason, usage}`, one row per
///   terminal assistant turn; `usage` is the whole turn's totals already.
///   Real-data anomalies: duplicate emissions sharing a `request_id`, and
///   degenerate `model: "<synthetic>"` zero-usage rows — both dropped.
/// - opencode — `{cost, reason, snapshot, tokens}`, one row per model
///   *step* (many per turn); `reason: "tool-calls"` marks an intermediate
///   step, anything else ends the turn. Values are per-step deltas, so a
///   turn's cost is the sum over its steps. There is no per-turn grouping
///   key in the payload (`source` is null, `snapshot` is a worktree hash),
///   so steps are accumulated per harness and flushed on the terminal
///   step's reason. Backfill can re-insert a live-written step a second
///   time (#93); token totals grow monotonically through a session, so a
///   byte-identical (harness, payload) repeat is a re-emission, never a
///   real step — dropped, or the sums double.
///
/// Shapes are disjoint, so detection is by payload keys, not harness kind.
export function costFromClaude(p: Payload): { tokens: number; usd: number } | undefined {
	if (str(p.model) === "<synthetic>") return undefined;
	const usage = obj(p.usage);
	if (!usage) return undefined;
	const tokens =
		(num(usage.input_tokens) ?? 0) +
		(num(usage.output_tokens) ?? 0) +
		(num(usage.cache_creation_input_tokens) ?? 0) +
		(num(usage.cache_read_input_tokens) ?? 0);
	return { tokens, usd: 0 };
}

export function stepFromOpencode(p: Payload): { tokens: number; usd: number; terminal: boolean } {
	const t = obj(p.tokens);
	const tokens =
		num(t?.total) ??
		(num(t?.input) ?? 0) +
			(num(t?.output) ?? 0) +
			(num(t?.reasoning) ?? 0) +
			(num(obj(t?.cache)?.read) ?? 0) +
			(num(obj(t?.cache)?.write) ?? 0);
	return { tokens, usd: num(p.cost) ?? 0, terminal: str(p.reason) !== "tool-calls" };
}
