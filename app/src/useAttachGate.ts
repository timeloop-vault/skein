// Live values the Claude transcript attach reads outside React's render (#763),
// split out of useTerminalSpawn.ts to keep it under the size limit.

import { useRef } from "react";

/** `live` is true only while the PTY is running: set after a live ensure,
 *  cleared on exit and on unmount. Refs, so the long-lived mount effect sees
 *  current values. */
export function useAttachGate(runsTool: boolean, claimSessionId: string | undefined) {
	const runsToolRef = useRef(runsTool);
	runsToolRef.current = runsTool;
	const claimRef = useRef(claimSessionId);
	claimRef.current = claimSessionId;
	const liveRef = useRef(false);
	return { runsToolRef, claimRef, liveRef };
}
