import type { VersionNoticeMode } from "./claudeVersion.ts";
import { claudeVersionStore, useVersionNotice } from "./claudeVersionStore.ts";
import { HarnessTab } from "./components.tsx";
import { effectiveStatus, useHarnessActivity } from "./harnessActivity.ts";
import { agentLabel, useObservedAgent } from "./harnessAgent.ts";
import type { GateResult } from "./harnessInputGate.ts";

// HarnessTab wrapper that subscribes to the activity store (epic #50) and
// the Claude Code update notice (#491); split out of HarnessColumn.tsx.

export const LiveHarnessTab = ({
	versionMode,
	onVersionRestart,
	...props
}: Parameters<typeof HarnessTab>[0] & {
	// #491: this kind's update-notice mode, and the in-place restart (#490).
	versionMode: VersionNoticeMode;
	onVersionRestart: (harnessId: string) => Promise<GateResult>;
}) => {
	const activity = useHarnessActivity(props.h.id);
	const agent = agentLabel(props.h, useObservedAgent(props.h.id));
	const { notice, refusal } = useVersionNotice(props.h.id);
	const update =
		versionMode !== "off" && notice
			? {
					notice,
					refusal,
					onRestart: () => {
						// Keep a refusal's reason for the tooltip and popover.
						void onVersionRestart(props.h.id).then((res) =>
							claudeVersionStore.setRefusal(props.h.id, res.ok ? null : res.reason),
						);
					},
				}
			: undefined;
	if (!activity) return <HarnessTab {...props} agent={agent} update={update} />;
	// Apply the acknowledged-downgrade: a waiting harness with no
	// pending notifications has already been seen, so render it as
	// idle (grey) instead of waiting (blue pulse). The phase in
	// the store stays `waiting` — only the visual indicator
	// collapses.
	const status = effectiveStatus(activity, props.h.pendingNotifications ?? 0);
	return <HarnessTab {...props} agent={agent} update={update} h={{ ...props.h, status }} />;
};
