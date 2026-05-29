// Activity card body. D1 renders a count + a tailing sentinel to
// prove the live subscription works; the full row catalogue,
// burst-collapse, turn separators, and auto-tail (handover §5.3, §6)
// land in D2.

export const ActivityCardBody = ({ count }: { count: number }) => {
	if (count === 0) {
		return (
			<div className="lc-empty">
				<div className="lc-empty-inner">
					<div className="big">☰</div>
					activity will appear here as agents work
				</div>
			</div>
		);
	}
	return (
		<div className="lc-activity">
			<div className="lc-activity-d1note">
				{count} action{count === 1 ? "" : "s"} captured — row rendering lands in the next slice
			</div>
			<div className="lc-tail">
				<span className="blinker" />
				tailing — new rows will appear live
			</div>
		</div>
	);
};
