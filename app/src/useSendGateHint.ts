// useSendGateHint — the thin React hook over `sendGateHint.ts` (#733 B).
//
// Returns `(body) => GateResult`, the daemon's last answer for that body's
// shape (`{ ok: true }` before the first answer). Refreshes on every phase
// mirror change for the harness; no work at all while disabled.

import { useCallback, useEffect, useState } from "react";
import { checkGate } from "./gateCheck.ts";
import { harnessActivity } from "./harnessActivity.ts";
import type { GateResult } from "./harnessInputGate.ts";
import { createSendGateHint, type SendGateHint } from "./sendGateHint.ts";

const OK: GateResult = { ok: true };

export function useSendGateHint(
	harnessId: string | null,
	enabled: boolean,
): (body: string) => GateResult {
	const [hint, setHint] = useState<SendGateHint | null>(null);
	const [version, setVersion] = useState(0);

	useEffect(() => {
		if (!enabled || harnessId === null) return undefined;
		const next = createSendGateHint((body) => checkGate(harnessId, body, "send"));
		const offHint = next.subscribe(() => setVersion((n) => n + 1));
		const offPhase = harnessActivity.subscribe(harnessId, () => next.refresh());
		setHint(next);
		next.refresh();
		return () => {
			offPhase();
			offHint();
			next.dispose();
			setHint(null);
		};
	}, [harnessId, enabled]);

	return useCallback(
		(body: string) => {
			void version;
			return hint ? hint.get(body) : OK;
		},
		[hint, version],
	);
}
