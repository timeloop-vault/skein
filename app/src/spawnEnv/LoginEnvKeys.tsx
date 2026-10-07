import { useState } from "react";
import "../SpawnEnvPanel.css";

const COLLAPSED_COUNT = 30;

/** Names of the variables a harness inherits from the login shell. Names
 *  only: the values may be secrets and never leave the backend. */
export const LoginEnvKeys = ({ keys }: { keys: string[] }) => {
	const [expanded, setExpanded] = useState(false);
	if (keys.length === 0) return null;
	const shown = expanded ? keys : keys.slice(0, COLLAPSED_COUNT);
	const hidden = keys.length - shown.length;
	return (
		<div className="sk-field">
			<label>From your login shell</label>
			<div className="sk-help">
				{keys.length} {keys.length === 1 ? "variable" : "variables"} your shell exports reach every
				harness. Values aren't shown.
			</div>
			<div className="sk-env-keys">
				{shown.map((k) => (
					<span className="sk-env-key" key={k}>
						{k}
					</span>
				))}
				{(hidden > 0 || expanded) && keys.length > COLLAPSED_COUNT && (
					<button type="button" className="sk-btn" onClick={() => setExpanded((e) => !e)}>
						{expanded ? "Show fewer" : `Show all (${hidden} more)`}
					</button>
				)}
			</div>
		</div>
	);
};
