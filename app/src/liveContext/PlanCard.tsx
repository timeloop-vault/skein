// Plan card body — issue #80 D4. One sub-list per harness (chip-headed),
// rows showing status box + text + opencode priority pill. Current state
// is reduced from the room's `plan_change` rows in plan.ts; this is the
// presentation only. Spec: handover §5.2. Lifted from the design canvas
// (docs/design/skein/project/live-context-cards.jsx PlanGroup/PlanItem).

import { HChip } from "../components.tsx";
import type { HarnessKind } from "../types.ts";
import type { PlanGroup, PlanItem } from "./plan.ts";

const PlanRow = ({ item }: { item: PlanItem }) => (
	<div className={`lc-plan-row ${item.status}${item.inferred ? " inferred" : ""}`}>
		<span className="box">{item.status === "done" ? "✓" : ""}</span>
		<span className="text">{item.text}</span>
		{item.priority && <span className={`pri ${item.priority}`}>{item.priority}</span>}
	</div>
);

export const PlanCardBody = ({
	groups,
	harnessKindOf,
	harnessNameOf,
}: {
	groups: PlanGroup[];
	harnessKindOf: (harnessId: string) => HarnessKind;
	/** Instance name for the group head; falls back to the kind label. */
	harnessNameOf: (harnessId: string) => string;
}) => {
	if (groups.length === 0) {
		return (
			<div className="lc-empty">
				<div className="lc-empty-inner">
					<div className="big">·</div>
					no plan items yet — agents will populate this as they work
				</div>
			</div>
		);
	}
	return (
		<div className="lc-plan">
			{groups.map((g) => {
				const done = g.items.filter((i) => i.status === "done").length;
				return (
					<div className="lc-plan-group" key={g.harnessId}>
						<div className="lc-plan-grouphead">
							<HChip kind={harnessKindOf(g.harnessId)} />
							<span>{harnessNameOf(g.harnessId)}</span>
							<span className="count">
								{done}/{g.items.length}
							</span>
						</div>
						{g.items.map((it) => (
							<PlanRow key={it.key} item={it} />
						))}
					</div>
				);
			})}
		</div>
	);
};
