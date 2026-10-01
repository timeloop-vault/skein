import type { EnvPreview } from "../types.ts";
import { DROP_REASON, PROBE_TONE, SOURCE_LABEL } from "./constants.ts";

/** The read-only half of the panel: probe status, the resolved PATH and
 *  the warnings the backend raised about the current settings. */
export const EnvPreviewSection = ({
	preview,
	busy,
	onReprobe,
}: {
	preview: EnvPreview | null;
	busy: boolean;
	onReprobe: () => void;
}) => {
	const probeTone = preview ? (PROBE_TONE[preview.probe.state] ?? "muted") : "muted";
	const missingPrograms = preview?.programs.filter((p) => p.resolved === null) ?? [];
	return (
		<>
			<div className="sk-help">
				Harnesses don't inherit your terminal's environment — a Skein launched from Finder or the
				Dock starts with a bare <code>PATH</code>. Skein asks your shell what it uses, then adds the
				directories below. This is what a harness spawned right now would get.
			</div>

			<div className="sk-env-status">
				<span className={`sk-env-dot sk-env-${probeTone}`} />
				<span className="sk-env-status-text">
					{preview
						? {
								captured: `PATH captured from ${preview.probe.shell} in ${preview.probe.elapsedMs} ms`,
								pending: "Asking your shell…",
								disabled: "PATH capture is turned off",
								not_applicable: "Using the live Windows registry PATH",
								unsupported_shell: "No PATH capture for this shell",
								timeout: "Your shell did not answer in time",
								spawn_failed: "Could not start your shell",
								no_payload: "Your shell answered with nothing usable",
							}[preview.probe.state]
						: "Reading…"}
				</span>
				<span className="sk-env-launch">
					launched from {preview?.launchContext === "terminal" ? "a terminal" : "the desktop"}
				</span>
				<button type="button" className="sk-btn" onClick={() => void onReprobe()} disabled={busy}>
					Re-probe
				</button>
			</div>

			{preview?.probe.message && <div className="sk-help">{preview.probe.message}</div>}

			{missingPrograms.length > 0 && (
				<div className="sk-env-banner sk-env-warn">
					Not on this PATH: {missingPrograms.map((p) => p.name).join(", ")}. A harness for one of
					these would fail to start.
				</div>
			)}

			<div className="sk-field">
				<label>Resolved PATH</label>
				<div className="sk-env-path">
					{preview?.path.map((row) => (
						<div
							className={`sk-env-path-row${row.exists ? "" : " sk-env-missing"}`}
							key={row.entry}
						>
							<span className={`sk-env-badge sk-env-badge-${row.source}`}>
								{SOURCE_LABEL[row.source] ?? row.source}
							</span>
							<span className="sk-env-path-entry">{row.entry}</span>
							{!row.exists && <span className="sk-env-note">missing</span>}
						</div>
					))}
				</div>
				<div className="sk-env-programs">
					{preview?.programs.map((p) => (
						<div className="sk-env-program" key={p.name}>
							<span className="sk-env-program-name">{p.name}</span>
							<span className={p.resolved ? "sk-env-program-ok" : "sk-env-program-missing"}>
								{p.resolved ?? "not found"}
							</span>
						</div>
					))}
				</div>
			</div>

			{preview && preview.droppedAdditions.length > 0 && (
				<div className="sk-env-banner sk-env-warn">
					Skipped:{" "}
					{preview.droppedAdditions
						.map((d) => `${d.entry} (${DROP_REASON[d.reason] ?? d.reason})`)
						.join(", ")}
				</div>
			)}

			{preview?.shellRejected && (
				<div className="sk-env-banner sk-env-warn">
					<code>{preview.shellRejected}</code> isn't a runnable file, so it's being ignored — Skein
					is using <code>{preview.shell}</code>.
				</div>
			)}

			{preview && preview.ignoredEnvKeys.length > 0 && (
				<div className="sk-env-banner sk-env-warn">
					Skein sets these itself, so your values are ignored: {preview.ignoredEnvKeys.join(", ")}.
					Use “Additional directories” to change PATH.
				</div>
			)}
		</>
	);
};
