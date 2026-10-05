// One harness's pane inside the column: PTY terminal, design preview (or
// its docked placeholder, #551) or files browser, chosen by capability.

import { DesignDockedPlaceholder } from "./DesignDockedPlaceholder.tsx";
import { DesignHarnessBody, type DeviceChangeHandler } from "./DesignHarnessBody.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import { FilesBody } from "./FilesBody.tsx";
import { LiveTerminal } from "./LiveTerminal.tsx";
import type { ShellClaimSink } from "./opencodeShellClaim.ts";
import type { Harness, Room } from "./types.ts";

interface HarnessBodyProps {
	harness: Harness;
	fontSize: number;
	// #158: copy-on-select setting — read live via a ref in LiveTerminal,
	// so toggling it applies to an already-running terminal without a
	// respawn. Threaded down exactly like fontSize.
	copyOnSelect: boolean;
	defaultShell: string[];
	visible: boolean;
	onCmdChange: (cmd: string[]) => void;
	// Stamped on every `harness_actions` row this harness emits
	// (issue #80). Threaded down to LiveTerminal → attachClaudeEvents.
	roomId: string;
	// Epic #50 L2c-2: opencode embedded-server port allocated by App.
	// `undefined` for non-opencode harnesses and for opencode harnesses
	// where pick_free_port failed — in the latter case the adapter
	// can't attach and the harness falls back to L2a.
	opencodePort: number | undefined;
	// SSE-capture callback: L2c-2 captures opencode's auto-allocated
	// sessionID from the `session.created` event. App.tsx wires it
	// to setHarnessSessionId; `undefined` for non-opencode harnesses.
	onSessionCaptured: ((sessionId: string) => void) | undefined;
	// #116: L2c-2 decided the harness followed its TUI onto a different
	// root session (`/new` or a `/sessions` pick). App.tsx wires it to
	// replaceHarnessSessionId, same as Claude's clear/resume/fork
	// follow; `undefined` for non-opencode harnesses.
	onSessionFollowed: ((sessionId: string) => void) | undefined;
	// #517: persists/releases an opencode followed from the post-exit shell.
	opencodeClaim: ShellClaimSink | undefined;
}

const HarnessBody = ({
	harness,
	fontSize,
	copyOnSelect,
	defaultShell,
	visible,
	onCmdChange,
	roomId,
	opencodePort,
	onSessionCaptured,
	onSessionFollowed,
	opencodeClaim,
}: HarnessBodyProps) => {
	if (harness.cmd && harness.cwd !== undefined) {
		// mountKey changes on cmd content OR spawnGen — the trigger for a
		// clean unmount + remount when the user picks Enter-for-shell
		// after a child exits. spawnGen is the explicit respawn signal:
		// the cmd-identity part alone misses a shell→shell respawn (new
		// cmd == old cmd → no remount → #53). Joining the array gives a
		// value-equal string across content-identical renders, so a
		// re-render with the same cmd + spawnGen doesn't churn the PTY.
		return (
			<LiveTerminal
				cmd={harness.cmd}
				cwd={harness.cwd}
				mountKey={`${harness.id}:${harness.spawnGen ?? 0}:${harness.cmd.join("\x00")}`}
				harnessId={harness.id}
				roomId={roomId}
				harnessKind={harness.kind}
				sessionId={harness.sessionId}
				agent={harness.agent}
				opencodePort={opencodePort}
				onSessionCaptured={onSessionCaptured}
				onSessionFollowed={onSessionFollowed}
				opencodeClaim={opencodeClaim}
				fontSize={fontSize}
				copyOnSelect={copyOnSelect}
				defaultShell={defaultShell}
				visible={visible}
				onCmdChange={onCmdChange}
			/>
		);
	}
	return null;
};

interface HarnessPaneBodyProps extends Omit<HarnessBodyProps, "roomId"> {
	room: Room;
	docked: boolean;
	onEntryChange: (roomId: string, harnessId: string, entry: string) => void;
	onDeviceChange: DeviceChangeHandler;
	onDesignDock: (harnessId: string, docked: boolean) => void;
	onDesignShow: (harnessId: string) => void;
}

export const HarnessPaneBody = ({
	room,
	docked,
	onEntryChange,
	onDeviceChange,
	onDesignDock,
	onDesignShow,
	...body
}: HarnessPaneBodyProps) => {
	const h = body.harness;
	if (HARNESS_KINDS[h.kind].capabilities.pty) return <HarnessBody {...body} roomId={room.id} />;
	if (h.kind === "design") {
		// Docked: the right pane owns the one pane for this id.
		if (docked) {
			return (
				<DesignDockedPlaceholder
					onShow={() => onDesignShow(h.id)}
					onUndock={() => onDesignDock(h.id, false)}
				/>
			);
		}
		return (
			<DesignHarnessBody
				room={room}
				harness={h}
				visible={body.visible}
				onEntryChange={onEntryChange}
				onDeviceChange={onDeviceChange}
				dock={{ docked: false, onToggle: () => onDesignDock(h.id, true) }}
			/>
		);
	}
	return <FilesBody harnessId={h.id} cwd={h.cwd ?? room.cwd ?? ""} visible={body.visible} />;
};
