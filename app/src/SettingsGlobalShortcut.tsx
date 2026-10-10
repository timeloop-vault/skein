// Settings → the OS-wide Control Center shortcut (#559).

import { GLOBAL_CONTROL_CENTER_LABEL } from "./shortcuts.ts";
import "./Settings.css";

export const GlobalShortcutSettings = ({
	enabled,
	failed,
	onChange,
}: {
	enabled: boolean;
	failed: boolean;
	onChange: (v: boolean) => void;
}) => (
	<div className="sk-field">
		<label>System-wide shortcut</label>
		<div className="sk-toggles">
			<label className="sk-toggle">
				<input type="checkbox" checked={enabled} onChange={(e) => onChange(e.target.checked)} />
				<span className="sk-toggle-label">
					<span className="sk-toggle-title">
						System-wide shortcut to raise the Control Center ({GLOBAL_CONTROL_CENTER_LABEL})
					</span>
					<span className="sk-toggle-sub">
						Works even when Skein is not focused. Opens the pop-out if it is closed.
					</span>
				</span>
			</label>
		</div>
		{enabled && failed && (
			<div className="sk-help">Couldn't register the shortcut — another app may be using it.</div>
		)}
	</div>
);
