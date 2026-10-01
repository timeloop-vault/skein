import type { EnvPreview, HarnessConfigStatus, SpawnSettings } from "../types.ts";
import "../Settings.css";
import "../SpawnEnvPanel.css";

/** The checkbox groups: agent review-tool injection and the agent-control
 *  kill switches, then the host-terminal stripping toggle. */
export const EnvToggles = ({
	draft,
	setDraft,
	harnessConfig,
	preview,
}: {
	draft: SpawnSettings;
	setDraft: (next: SpawnSettings) => void;
	harnessConfig: HarnessConfigStatus | null;
	preview: EnvPreview | null;
}) => (
	<>
		<div className="sk-field">
			<label>Review tools in your agents</label>
			<div className="sk-help">
				So an agent can read your review comments (#215), Skein points each CLI at a small
				configuration it ships. Both are <strong>session-scoped and additive</strong> — nothing is
				written into the worktree, and your own plugins, connectors and config are left alone. A
				repo's own <code>opencode.json</code> still wins over Skein's.
			</div>
			{harnessConfig?.error && (
				<div className="sk-env-banner sk-env-err">
					{harnessConfig.error} — agents in every room will have no review tools.
				</div>
			)}
			<div className="sk-toggles">
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.injectClaudePlugin}
						disabled={!harnessConfig?.claudePlugin}
						onChange={(e) => setDraft({ ...draft, injectClaudePlugin: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Claude Code</span>
						<span className="sk-toggle-sub">
							{harnessConfig?.claudePlugin ? (
								<>
									Appends{" "}
									<code>
										{harnessConfig.claudeFlag} {harnessConfig.claudePlugin}
									</code>{" "}
									to the command. Loads for that session only — nothing is installed, and a plugin
									you installed yourself is untouched unless it is also named <code>skein</code>.
								</>
							) : (
								<>The shipped plugin didn't resolve, so there is nothing to inject.</>
							)}
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.injectOpencodeConfig}
						disabled={!harnessConfig?.opencodeConfig}
						onChange={(e) => setDraft({ ...draft, injectOpencodeConfig: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">opencode</span>
						<span className="sk-toggle-sub">
							{harnessConfig?.opencodeConfig ? (
								<>
									Sets{" "}
									<code>
										{harnessConfig.opencodeVar}={harnessConfig.opencodeConfig}
									</code>
									, which opencode merges between your global config and the project's. Turn this
									off to use that variable for a config file of your own.
								</>
							) : (
								<>The shipped config didn't resolve, so there is nothing to inject.</>
							)}
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.allowAgentMessaging}
						onChange={(e) => setDraft({ ...draft, allowAgentMessaging: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Let agents message other harnesses</span>
						<span className="sk-toggle-sub">
							When off, an agent's <code>send_message</code> and <code>read_messages</code> calls
							are refused with the reason.
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.allowAgentRoomCreation}
						onChange={(e) => setDraft({ ...draft, allowAgentRoomCreation: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Let agents open rooms</span>
						<span className="sk-toggle-sub">
							When off, an agent's <code>create_room</code> calls are refused with the reason.
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.allowAgentRoomClosing}
						onChange={(e) => setDraft({ ...draft, allowAgentRoomClosing: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Let agents close rooms they created</span>
						<span className="sk-toggle-sub">
							When off, an agent's <code>close_room</code> calls are refused with the reason.
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.allowAgentHarnessControl}
						onChange={(e) => setDraft({ ...draft, allowAgentHarnessControl: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Let agents open or close harnesses</span>
						<span className="sk-toggle-sub">
							When off, an agent's <code>open_harness</code> and <code>close_harness</code> calls
							are refused with the reason.
						</span>
					</span>
				</label>
			</div>
		</div>

		<div className="sk-field">
			<div className="sk-toggles">
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.stripHostEnv}
						onChange={(e) => setDraft({ ...draft, stripHostEnv: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Hide the host terminal from harnesses</span>
						<span className="sk-toggle-sub">
							When Skein is started from tmux, VS Code or Windows Terminal, markers like
							TERM_PROGRAM and TMUX leak into the agent CLIs — which sniff them and adopt the host
							terminal's key and clipboard behaviour instead of Skein's.
							{preview && preview.stripped.length > 0 && (
								<> Currently hiding: {preview.stripped.join(", ")}.</>
							)}
						</span>
					</span>
				</label>
			</div>
		</div>
	</>
);
