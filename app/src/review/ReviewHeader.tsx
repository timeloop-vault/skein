// The review pane's header: base picker, scope switch, counts, the
// accept-all / nudge / sign-off controls. Split out of ReviewPane.tsx
// (#19) — moved verbatim.

import type { HarnessCapabilities } from "../data.tsx";
import type { GateResult } from "../harnessInput.ts";
import { acceptReview } from "../liveContext/review.ts";
import { SignoffControl, type SignoffIntent } from "./SignoffControl.tsx";
import { type ReviewScope, type ReviewScopeData, setBaseRef } from "./api.ts";
import type { Nudge } from "./nudges.ts";
import type { SignoffStatus } from "./signoff.ts";

const SCOPES: Array<{ id: ReviewScope; label: string; title: string }> = [
	{
		id: "branch",
		label: "branch",
		title: "everything this branch does — committed and uncommitted, against the base",
	},
	{ id: "commit", label: "commits", title: "read the branch one commit at a time" },
	{
		id: "pending",
		label: "pending",
		title: "uncommitted work only — the one scope you can accept or reject",
	},
];

export const ReviewHeader = ({
	roomId,
	cwd,
	data,
	setData,
	busy,
	run,
	scope,
	setScope,
	setActivePath,
	filesCount,
	activeCapabilities,
	nudge,
	nudgeGate,
	nudgeDisabledReason,
	onNudge,
	signoff,
	signoffBusy,
	signoffIntent,
	setSignoffIntent,
}: {
	roomId: string;
	cwd: string;
	data: ReviewScopeData | undefined;
	setData: (next: ReviewScopeData) => void;
	busy: boolean;
	run: (work: () => Promise<unknown>) => void;
	scope: ReviewScope;
	setScope: (s: ReviewScope) => void;
	setActivePath: (p: string | undefined) => void;
	/** `files.length` — whether the scope's own "accept all" button shows. */
	filesCount: number;
	activeCapabilities: HarnessCapabilities | null;
	nudge: Nudge | undefined;
	nudgeGate: GateResult | undefined;
	nudgeDisabledReason: string | undefined;
	onNudge: () => void;
	signoff: SignoffStatus | undefined;
	signoffBusy: boolean;
	signoffIntent: SignoffIntent | undefined;
	setSignoffIntent: (intent: SignoffIntent | undefined) => void;
}) => (
	<div className="rv-head">
		<div className="rv-head-row">
			{data?.isRepo ? (
				<label className="rv-base">
					<span className="rv-base-label">base</span>
					<select
						className="rv-select"
						value={data.baseRef ?? ""}
						disabled={busy}
						onChange={(e) => {
							const next = e.target.value;
							if (!next) return;
							run(() => setBaseRef(roomId, cwd, next).then(setData));
						}}
					>
						{/* A base that no longer resolves still shows, so the
						    header explains the empty review rather than
						    silently swapping in a different branch. */}
						{data.baseRef && !data.branches.includes(data.baseRef) && (
							<option value={data.baseRef}>{data.baseRef}</option>
						)}
						{data.branches.map((b) => (
							<option key={b} value={b}>
								{b}
							</option>
						))}
					</select>
				</label>
			) : (
				<span className="rv-nonrepo">not a git repository</span>
			)}
			<span className="rv-counts">
				{data?.headBranch && <span className="rv-branch">{data.headBranch}</span>}
				{data && data.commits.length > 0 && (
					<span>
						{data.commits.length} commit{data.commits.length === 1 ? "" : "s"}
					</span>
				)}
				{data && data.files.length > 0 && <span>{data.files.length} files</span>}
				{data && data.additions > 0 && <span className="delta-add">+{data.additions}</span>}
				{data && data.deletions > 0 && <span className="delta-del">−{data.deletions}</span>}
			</span>
		</div>

		<div className="rv-head-row">
			<span className="rv-scopes">
				{SCOPES.map((s) => (
					<button
						type="button"
						key={s.id}
						className={`rv-scope${scope === s.id ? " on" : ""}`}
						title={s.title}
						onClick={() => {
							setScope(s.id);
							setActivePath(undefined);
						}}
					>
						{s.label}
						{s.id === "pending" && data && data.pendingCount > 0 && (
							<span className="rv-badge open">{data.pendingCount}</span>
						)}
					</button>
				))}
			</span>
			<span className="rv-head-right">
				{data && data.unresolvedCount > 0 && (
					<span className="rv-unresolved" title="unresolved comment threads in this review">
						{data.unresolvedCount} open
					</span>
				)}
				{scope === "pending" && filesCount > 0 && (
					<button
						type="button"
						className="rv-act all"
						disabled={busy}
						title="accept every pending file — advances the baseline, writes nothing"
						onClick={() => run(() => acceptReview(roomId, cwd, undefined, [], undefined))}
					>
						✓ all
					</button>
				)}
				{/* #238: pastes one of three fixed prompts into the room's
				    active harness and submits it. No button at all when
				    that harness has no terminal to paste into — #41's
				    file drop will read the same seam.
				    #355: the tooltip is `data-sk-tip` on a WRAPPING span,
				    not `title` on the button — the styled hover popover
				    (statusPopover.ts) replaces the native box, and a
				    disabled button fires no mouse events at all in
				    Chromium, so the trigger has to live on an ancestor. */}
				{activeCapabilities?.pty && (
					<span
						className="rv-act-tip"
						data-sk-tip={nudgeDisabledReason ?? nudge?.body}
						aria-label={nudgeDisabledReason ?? nudge?.body}
					>
						<button
							type="button"
							className="rv-act nudge"
							disabled={!nudge || !nudgeGate?.ok}
							onClick={onNudge}
						>
							{nudge?.label ?? "Nudge"}
						</button>
					</span>
				)}
				{/* #214: the terminal act of a review. Not a merge and not
				    a push — Skein does neither. This records that the
				    reviewer approved this commit, which is what the agent
				    checks before landing the branch its own way. */}
				<SignoffControl
					status={signoff}
					busy={signoffBusy}
					pending={signoffIntent}
					onRequest={setSignoffIntent}
				/>
			</span>
		</div>
	</div>
);
