// Over-threshold marks (#368): a harness is over when its latest-turn context
// size exceeds the user's threshold. Snapshots are booleans so
// useSyncExternalStore sees a stable value.

import { useSyncExternalStore } from "react";
import { anyOver, isOverThreshold } from "./contextSize.ts";
import { getContextThreshold, subscribeContextThreshold } from "./contextThresholdStore.ts";
import { getHarnessContextSize, subscribeContextSizes } from "./useHarnessContextSizes.ts";

function subscribeBoth(cb: () => void): () => void {
	const a = subscribeContextSizes(cb);
	const b = subscribeContextThreshold(cb);
	return () => {
		a();
		b();
	};
}

export function useHarnessOverThreshold(harnessId: string): boolean {
	return useSyncExternalStore(subscribeBoth, () =>
		isOverThreshold(getHarnessContextSize(harnessId), getContextThreshold()),
	);
}

export function useAnyHarnessOverThreshold(harnessIds: readonly string[]): boolean {
	return useSyncExternalStore(subscribeBoth, () =>
		anyOver(harnessIds, getHarnessContextSize, getContextThreshold()),
	);
}
