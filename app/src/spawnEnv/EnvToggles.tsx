import type { EnvPreview, HarnessConfigStatus, SpawnSettings } from "../types.ts";
import { ToolGroupToggles } from "./ToolGroupToggles.tsx";
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
									to the command. In the shell after Claude exits it sets{" "}
									<code>
										{harnessConfig.claudeShellVar}={harnessConfig.claudePlugin}
									</code>{" "}
									instead, added to any value you already have. Loads for that session only —
									nothing is installed, and a plugin you installed yourself is untouched unless it
									is also named <code>skein</code>.
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
									, which opencode merges between your global config and the project's. The shell
									after opencode exits gets it too. Turn this off to use that variable for a config
									file of your own.
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
						checked={draft.injectHermesConfig}
						onChange={(e) => setDraft({ ...draft, injectHermesConfig: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Hermes</span>
						<span className="sk-toggle-sub">
							Skein's own profile <code>{harnessConfig?.hermesProfile ?? "skein"}</code> carries the
							review tools: an MCP entry in{" "}
							<code>
								{harnessConfig?.hermesRoot ? `${harnessConfig.hermesRoot}/` : ""}profiles/
								{harnessConfig?.hermesProfile ?? "skein"}/config.yaml
							</code>
							. Your own Hermes config is never changed unless you agree when picking one of your
							profiles. Turning this off removes the entry on the next start. That profile does not
							read your Hermes <code>.env</code>: put your provider key (e.g.{" "}
							<code>OPENROUTER_API_KEY</code>) in your shell environment or in the environment
							below.
						</span>
					</span>
				</label>
			</div>
		</div>

		<ToolGroupToggles draft={draft} setDraft={setDraft} />

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
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.disableClaudeWheelAcceleration}
						onChange={(e) =>
							setDraft({ ...draft, disableClaudeWheelAcceleration: e.target.checked })
						}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Turn off Claude Code's scroll acceleration</span>
						<span className="sk-toggle-sub">
							Claude Code speeds up wheel scrolling while scroll events arrive close together, and a
							trackpad sits right on its threshold, so the speed keeps jumping. On, Skein passes{" "}
							<code>--settings {'{"wheelScrollAccelerationEnabled":false}'}</code> to each Claude
							harness: one line per line of travel. This overrides that setting in your own Claude
							Code config, inside Skein only.
						</span>
					</span>
				</label>
				<label className="sk-toggle">
					<input
						type="checkbox"
						checked={draft.lowerHarnessPriority}
						onChange={(e) => setDraft({ ...draft, lowerHarnessPriority: e.target.checked })}
					/>
					<span className="sk-toggle-label">
						<span className="sk-toggle-title">Run harnesses at lower CPU priority</span>
						<span className="sk-toggle-sub">
							Harnesses and everything they start — builds, tests, language servers — run at
							below-normal priority (Windows) or nice 10 (macOS/Linux), so a busy build yields to
							Skein and the rest of your desktop instead of freezing it. Applies to harnesses
							started after you change it; running ones keep their priority until restarted.
						</span>
					</span>
				</label>
			</div>
		</div>
	</>
);
