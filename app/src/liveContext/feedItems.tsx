// Flattened-item layer for the Activity feed — issue #80 D2d. Split (#460)
// into feedItemTypes.ts (the model), feedFlatten.ts (flattenFeed),
// feedTotals.ts (session totals) and feedItemViews.tsx (components);
// importers keep using this path.

export { flattenFeed } from "./feedFlatten.ts";
export type { FeedItem, FlattenOptions } from "./feedItemTypes.ts";
export {
	BackfillBanner,
	BackfillEnd,
	BurstRow,
	SessionTotals,
	TurnCost,
	TurnSeparator,
} from "./feedItemViews.tsx";
export { formatTokensShort, sessionTotals } from "./feedTotals.ts";
