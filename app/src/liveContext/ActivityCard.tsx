// Activity card body. D2a renders every action as its row (via the
// ActivityRow dispatcher) plus a tailing sentinel. Turn separators,
// per-turn cost hair-lines, the backfill banner, burst-collapse, and
// auto-tail land in D2d–D2f; virtualization in D2g.

import type { HarnessKind } from "../types.ts";
import { ActivityRow } from "./rows.tsx";
import type { HarnessAction } from "./store.ts";

export const ActivityCardBody = ({
	actions,
	harnessKindOf,
}: {
	actions: HarnessAction[];
	harnessKindOf: (harnessId: string) => HarnessKind;
}) => {
	if (actions.length === 0) {
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
			{actions.map((row) => (
				<ActivityRow key={row.id} row={row} harnessKindOf={harnessKindOf} />
			))}
			<div className="lc-tail">
				<span className="blinker" />
				tailing — new rows appear live
			</div>
		</div>
	);
};
