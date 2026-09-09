// The sign-off control (#214, epic #52 D9 as corrected).
//
// The end of the review, and the only thing in the pane the agent
// treats as permission. Three states, because collapsing any two would
// undo the point of the feature:
//
//   none      — not approved. The agent waits.
//   approved  — approved, and HEAD is still what was approved.
//   stale     — approved once; something has been committed since, so
//               the approval no longer covers what is on the branch.
//
// Skein does not merge, push, or open pull requests. The agent does
// that, following conventions it can read out of the repository and
// Skein could only guess at — which strategy, which forge, which PR
// body. What Skein owns is the one fact that lives nowhere else:
// whether the human said yes, and to exactly which commit.
//
// Confirmation is an inline row, not `plugin-dialog`'s `confirm`. That
// one draws a native OS message box — right for closing a room, where
// PTYs die and unsaved buffers are discarded, and wrong here: signing
// off is small, in-pane and reversible in a click, and a Win32 dialog
// in the middle of a dark app is a bigger interruption than the action
// deserves.
//
// Presentational; `useSignoff` owns the status and `ReviewPane` owns
// the pending intent, because the button lives in the header and the
// confirmation renders below it.

import {
	type SignoffStatus,
	approvePrompt,
	short,
	signoffState,
	staleExplanation,
	withdrawPrompt,
} from "./signoff.ts";

/// Which confirmation is open, if any.
export type SignoffIntent = "approve" | "withdraw";

const ago = (ms: number): string => {
	const diff = Date.now() - ms;
	if (diff < 60_000) return "just now";
	if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} min ago`;
	if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} h ago`;
	return `${Math.floor(diff / 86_400_000)} d ago`;
};

export const SignoffControl = ({
	status,
	busy,
	pending,
	onRequest,
}: {
	status: SignoffStatus | undefined;
	busy: boolean;
	pending: SignoffIntent | undefined;
	onRequest: (intent: SignoffIntent | undefined) => void;
}) => {
	// A room with no repository has no commit to approve, so the
	// control is absent rather than present-and-broken.
	if (!status?.canSignOff) return null;

	// Clicking the button again closes the confirmation it opened —
	// the same affordance both ways, so it never becomes a trap.
	const toggle = (intent: SignoffIntent) => onRequest(pending === intent ? undefined : intent);
	const armed = pending ? " armed" : "";

	switch (signoffState(status)) {
		case "approved":
			return (
				<button
					type="button"
					className={`rv-signoff approved${armed}`}
					disabled={busy}
					title={`signed off on ${short(status.approvedSha)}${
						status.approvedMs ? ` ${ago(status.approvedMs)}` : ""
					} — click to withdraw`}
					onClick={() => toggle("withdraw")}
				>
					✓ signed off
				</button>
			);
		case "stale":
			return (
				<button
					type="button"
					className={`rv-signoff stale${armed}`}
					disabled={busy}
					title={staleExplanation(status)}
					onClick={() => toggle("approve")}
				>
					⚠ sign-off lapsed
				</button>
			);
		default:
			return (
				<button
					type="button"
					className={`rv-signoff${armed}`}
					disabled={busy || !status.headSha}
					title={
						status.headSha
							? `sign off on ${short(status.headSha)} — the agent reads this as clearance to land`
							: "this branch has no commits yet, so there is nothing to sign off on"
					}
					onClick={() => toggle("approve")}
				>
					○ sign off
				</button>
			);
	}
};

/// The confirmation, inline under the header.
///
/// Below the header rather than floating over the diff: the pane is one
/// narrow column, and a popover here would cover the very thing the
/// reviewer is signing off on.
export const SignoffConfirm = ({
	status,
	pending,
	busy,
	onConfirm,
	onCancel,
}: {
	status: SignoffStatus | undefined;
	pending: SignoffIntent | undefined;
	busy: boolean;
	onConfirm: (approved: boolean) => void;
	onCancel: () => void;
}) => {
	if (!pending || !status) return null;
	const approving = pending === "approve";
	const prompt = approving ? approvePrompt(status) : withdrawPrompt();

	return (
		<div className="rv-signoff-confirm">
			<div className="rv-signoff-ask">
				<span className="q">{prompt.question}</span>
				<span className="detail">{prompt.detail}</span>
				{prompt.openThreads && <span className="warn">{prompt.openThreads}</span>}
			</div>
			<div className="rv-signoff-buttons">
				<button
					type="button"
					className="sk-btn primary"
					disabled={busy}
					onClick={() => onConfirm(approving)}
				>
					{approving ? "sign off" : "withdraw"}
				</button>
				<button type="button" className="sk-btn" disabled={busy} onClick={onCancel}>
					cancel
				</button>
			</div>
		</div>
	);
};

/// The lapsed banner. Separate from the confirmation because a sign-off
/// that stopped applying is a standing fact, not a question — the
/// reviewer has to see it whether or not they are mid-decision.
export const SignoffNotice = ({ status }: { status: SignoffStatus | undefined }) => {
	if (!status?.stale) return null;
	return <div className="rv-signoff-notice">sign-off lapsed — {staleExplanation(status)}</div>;
};
