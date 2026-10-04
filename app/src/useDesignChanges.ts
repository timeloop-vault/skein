// useDesignChanges — `design.show_changes` (#547) against the preview frame:
// ask for its rendered source sites, match them to the changed lines
// (designChanges.ts), ask the frame to outline the matches. Never touches
// focus, the selection or the active room.

import { useCallback, useRef } from "react";
import {
	changesAnswer,
	MAX_OUTLINED,
	matchChanges,
	type ShowChangesRequest,
	type ShowChangesResult,
} from "./designChanges.ts";
import type { Beacon, HostMessage } from "./designPreview.ts";
import { PendingRequests } from "./designShow.ts";

type Sources = Extract<Beacon, { type: "sources" }>;
type Shown = Extract<Beacon, { type: "shownChanges" }>;

export const useDesignChanges = ({
	ready,
	post,
}: {
	/** Latest readiness, read at call time. */
	ready: () => boolean;
	post: (msg: HostMessage) => void;
}) => {
	const sources = useRef(new PendingRequests<Sources>("agent-src-")).current;
	const shown = useRef(new PendingRequests<Shown>("agent-chg-")).current;
	const postRef = useRef(post);
	postRef.current = post;
	const readyRef = useRef(ready);
	readyRef.current = ready;

	const showChanges = useCallback(
		async (req: ShowChangesRequest): Promise<ShowChangesResult> => {
			if (!readyRef.current()) throw new Error("not_ready: the design page has not loaded yet");
			const listed = await sources.begin((requestId) =>
				postRef.current({ type: "listSources", requestId }),
			);
			const match = matchChanges(listed.sites, req.files, listed.capped === true);
			const out = await shown.begin((requestId) =>
				postRef.current({
					type: "showChanges",
					requestId,
					sites: match.matchedSites,
					max: MAX_OUTLINED,
				}),
			);
			return changesAnswer(match, out, req.truncated === true);
		},
		[sources, shown],
	);

	/** True when the beacon answered one of our requests (or is ours). */
	const onBeacon = useCallback(
		(b: Beacon): boolean => {
			if (b.type === "sources") {
				sources.settle(b.requestId, b);
				return true;
			}
			if (b.type === "shownChanges") {
				shown.settle(b.requestId, b);
				return true;
			}
			return b.type === "changesCleared";
		},
		[sources, shown],
	);

	const rejectAll = useCallback(
		(why: string) => {
			sources.rejectAll(why);
			shown.rejectAll(why);
		},
		[sources, shown],
	);

	return { showChanges, onBeacon, rejectAll };
};
