// Rows for background tasks (#447). A task's start is already a feed row
// (the Bash/Monitor tool_call); this is the other end, from the
// `background_end` harness_actions row. Monitor per-event rows are
// deliberately not rendered — noise.

import type { HarnessKind } from "../types.ts";
import { backgroundEndView } from "./backgroundEnd.ts";
import type { Payload } from "./payload.ts";
import { formatDuration, Row } from "./Row.tsx";

export const BackgroundEndRow = ({
	payload,
	harness,
	timestampMs,
}: {
	payload: Payload;
	harness: HarnessKind | null | undefined;
	timestampMs: number;
}) => {
	const view = backgroundEndView(payload);
	return (
		<Row
			kind="background"
			glyph="⧗"
			harness={harness}
			timestampMs={timestampMs}
			right={
				view.durationMs != null ? (
					<span className="dim">{formatDuration(view.durationMs)}</span>
				) : undefined
			}
		>
			<span className="tool">{view.kind}</span> <span className="target">{view.target}</span>{" "}
			<span className={view.failed ? "err-text" : "dim"}>{view.outcome}</span>
		</Row>
	);
};
