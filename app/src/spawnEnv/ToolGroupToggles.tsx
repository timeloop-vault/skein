import type { SpawnSettings } from "../types.ts";
import { groupEnabled, TOOL_GROUPS, withGroup } from "./toolGroups.ts";
import "../Settings.css";
import "../SpawnEnvPanel.css";

/** One checkbox per MCP tool group (#534). */
export const ToolGroupToggles = ({
	draft,
	setDraft,
}: {
	draft: SpawnSettings;
	setDraft: (next: SpawnSettings) => void;
}) => (
	<div className="sk-field">
		<label>Skein tools for agents</label>
		<div className="sk-help">
			A group that is off is hidden from every harness's Skein tool list and refused if called.
			Running harnesses see the changed list after a restart; refusals apply immediately. To
			restrict tools for one agent only, use that agent's own definition (its tools allowlist).
		</div>
		<div className="sk-toggles">
			{TOOL_GROUPS.map((g) => (
				<label key={g.key} className="sk-toggle">
					<input
						type="checkbox"
						checked={groupEnabled(draft, g.key)}
						onChange={(e) => setDraft(withGroup(draft, g.key, e.target.checked))}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">{g.label}</span>
						<span className="sk-toggle-sub">
							{g.tools.map((t, i) => (
								<span key={t}>
									{i > 0 && ", "}
									<code>{t}</code>
								</span>
							))}
							{g.tools.length > 0 && ". "}
							{g.note}
						</span>
					</span>
				</label>
			))}
		</div>
	</div>
);
