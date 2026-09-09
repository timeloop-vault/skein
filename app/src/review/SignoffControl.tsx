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
// Presentational; `useSignoff` owns the state, because the lapsed
// notice below the header needs the same status this button does.

import { confirm } from "@tauri-apps/plugin-dialog";
import {
	type SignoffStatus,
	approveConfirmation,
	short,
	signoffState,
	staleExplanation,
} from "./signoff.ts";

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
	onSet,
}: {
	status: SignoffStatus | undefined;
	busy: boolean;
	onSet: (approved: boolean, note?: string) => void;
}) => {
	// A room with no repository has no commit to approve, so the
	// control is absent rather than present-and-broken.
	if (!status?.canSignOff) return null;

	const approve = async () => {
		if (
			await confirm(approveConfirmation(status), {
				title: "Sign off on this review",
				kind: "info",
			})
		) {
			onSet(true);
		}
	};

	const withdraw = async () => {
		if (
			await confirm(
				"Withdraw your sign-off?\n\nThe agent will read this as no longer cleared to land.",
				{ title: "Withdraw sign-off", kind: "warning" },
			)
		) {
			onSet(false);
		}
	};

	switch (signoffState(status)) {
		case "approved":
			return (
				<button
					type="button"
					className="rv-signoff approved"
					disabled={busy}
					title={`signed off on ${short(status.approvedSha)}${
						status.approvedMs ? ` ${ago(status.approvedMs)}` : ""
					} — click to withdraw`}
					onClick={() => void withdraw()}
				>
					✓ signed off
				</button>
			);
		case "stale":
			return (
				<button
					type="button"
					className="rv-signoff stale"
					disabled={busy}
					title={staleExplanation(status)}
					onClick={() => void approve()}
				>
					⚠ sign-off lapsed
				</button>
			);
		default:
			return (
				<button
					type="button"
					className="rv-signoff"
					disabled={busy || !status.headSha}
					title={
						status.headSha
							? `sign off on ${short(status.headSha)} — the agent reads this as clearance to land`
							: "this branch has no commits yet, so there is nothing to sign off on"
					}
					onClick={() => void approve()}
				>
					○ sign off
				</button>
			);
	}
};

/// The lapsed banner. In the body rather than the header because a
/// sign-off that stopped applying is the one state the reviewer has to
/// notice, and a tooltip on a button is not noticing.
export const SignoffNotice = ({ status }: { status: SignoffStatus | undefined }) => {
	if (!status?.stale) return null;
	return <div className="rv-signoff-notice">sign-off lapsed — {staleExplanation(status)}</div>;
};
