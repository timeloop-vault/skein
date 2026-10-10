// #568: the picker's step for a `remote` harness — host, tool and
// an optional directory, instead of an agent list.

import { useState } from "react";
import { loadRemoteLast, saveRemoteLast } from "./prefs.ts";
import { type RemoteInput, validateRemoteHost } from "./remoteCmd.ts";
import "./RemoteHostStep.css";

export const RemoteHostStep = ({
	onSubmit,
	onBack,
}: {
	onSubmit: (input: RemoteInput) => void;
	onBack: () => void;
}) => {
	const [last] = useState(loadRemoteLast);
	const [host, setHost] = useState(last.host);
	const [tool, setTool] = useState(last.tool);
	const [dir, setDir] = useState("");
	const [touched, setTouched] = useState(false);

	const hostError = validateRemoteHost(host);
	const toolError = tool.trim() === "" ? "Tool is required." : null;
	const error = hostError ?? toolError;
	const submit = () => {
		setTouched(true);
		if (error) return;
		saveRemoteLast({ host: host.trim(), tool: tool.trim() });
		onSubmit({ host, tool, ...(dir.trim() ? { dir } : {}) });
	};
	const onKeyDown = (e: React.KeyboardEvent) => {
		if (e.key === "Enter") submit();
		if (e.key === "Escape") onBack();
	};

	return (
		<>
			<h3>remote — which host?</h3>
			<p>
				Runs the tool inside tmux on the host over ssh. The session survives a Skein restart and is
				reattached.
			</p>
			<div className="sk-remote-form">
				<input
					// biome-ignore lint/a11y/noAutofocus: the step's only purpose is these fields
					autoFocus
					className="sk-input"
					placeholder="user@host or ssh alias"
					value={host}
					onChange={(e) => setHost(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				<input
					className="sk-input"
					placeholder="remote command"
					value={tool}
					onChange={(e) => setTool(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				<div className="sk-remote-hint">remote command, e.g. opencode or ~/.local/bin/claude</div>
				<input
					className="sk-input"
					placeholder="remote dir (optional, absolute)"
					value={dir}
					onChange={(e) => setDir(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				{touched && error && <div className="sk-remote-error">{error}</div>}
			</div>
			<div className="sk-remote-actions">
				<button className="sk-btn" type="button" onClick={onBack}>
					← Back
				</button>
				<button className="sk-btn" type="button" onClick={submit}>
					Add remote harness
				</button>
			</div>
		</>
	);
};
