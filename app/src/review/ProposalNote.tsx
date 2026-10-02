// The structured change request a proposal thread (#436) carries. Skein
// never applies it: the agent reads it through the review API, edits the
// source and marks the thread addressed. Every value is page content and
// therefore untrusted — plain text only, clipped.

import type { ReviewThread } from "./api.ts";
import "./element.css";

export const ProposalNote = ({ thread }: { thread: ReviewThread }) => {
	const proposal = thread.proposal;
	if (!proposal) return null;
	return (
		<div className="rv-proposal">
			<div className="rv-proposal-head">
				<span className="rv-proposal-tag">proposed edit</span>
				{thread.addressed && <span className="rv-addressed-tag">addressed</span>}
			</div>
		</div>
	);
};
