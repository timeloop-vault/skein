// Settings → Claude Code update notice (#491).

import type { VersionNoticeMode } from "./claudeVersion.ts";
import "./Settings.css";

const OPTIONS: { mode: VersionNoticeMode; label: string }[] = [
	{ mode: "off", label: "Off" },
	{ mode: "badge", label: "Badge" },
	{ mode: "auto", label: "Auto-restart" },
];

export const VersionNoticeSettings = ({
	mode,
	onChange,
}: {
	mode: VersionNoticeMode;
	onChange: (mode: VersionNoticeMode) => void;
}) => (
	<div className="sk-field">
		<label>Claude Code updates</label>
		<div className="sk-help">
			Badge marks a harness running an older Claude Code with a Restart action. Auto-restart resumes
			the conversation in a fresh process once the harness has been waiting for you for a few
			seconds, so a question it just asked stays in the history but any open picker or dialog is
			gone.
		</div>
		<div className="sk-radio-row">
			{OPTIONS.map((o) => (
				<button
					key={o.mode}
					type="button"
					className={`sk-radio-card ${mode === o.mode ? "selected" : ""}`}
					onClick={() => onChange(o.mode)}
				>
					<div className="top">{o.label}</div>
				</button>
			))}
		</div>
	</div>
);
