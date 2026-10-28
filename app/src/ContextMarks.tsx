// Over-threshold marks (#368). Visual only: no notification, no badge count.

import { formatContextTokens } from "./contextSize.ts";
import { useContextThreshold } from "./contextThresholdStore.ts";
import { useAnyHarnessOverThreshold, useHarnessOverThreshold } from "./useContextThreshold.ts";
import { useHarnessContextSize } from "./useHarnessContextSizes.ts";
import "./ContextMarks.css";

/** Amber chip on a harness tab, e.g. "162k"; renders nothing unless over. */
export const ContextChip = ({ harnessId }: { harnessId: string }) => {
	const over = useHarnessOverThreshold(harnessId);
	const size = useHarnessContextSize(harnessId);
	const threshold = useContextThreshold();
	if (!over || size === undefined || threshold === null) return null;
	const text = `Context ${formatContextTokens(size)} is above your ${formatContextTokens(threshold)} threshold`;
	return (
		<span className="tab-ctx" title={text}>
			{formatContextTokens(size)}
		</span>
	);
};

/** Small amber pill on a room or group tab when any listed harness is over. */
export const ContextMark = ({ harnessIds }: { harnessIds: readonly string[] }) => {
	const over = useAnyHarnessOverThreshold(harnessIds);
	if (!over) return null;
	const text = "A harness is over the context threshold";
	return (
		<span className="tab-ctx tab-ctx--room" title={text}>
			ctx
		</span>
	);
};
