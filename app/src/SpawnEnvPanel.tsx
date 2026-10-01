// Settings → Shell & environment (issues #72, #3, #1).
//
// The problem this exists to solve is not really "PATH is wrong" — it
// is that the resolved harness environment was observable NOWHERE. The
// only record was a log line in a daily-rotating file, and it printed
// the probe's output, i.e. the value *before* Skein's own additions. So
// "a tool works in my terminal but not in a harness" was unanswerable
// by inspection, and the only fix for a missing directory was a code
// change and a release.
//
// Hence the shape: the preview comes first and is built by the same
// Rust that builds a real child's environment, then the knobs.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { EnvPreviewSection } from "./spawnEnv/EnvPreviewSection.tsx";
import { EnvToggles } from "./spawnEnv/EnvToggles.tsx";
import { CAPTURE_OPTIONS } from "./spawnEnv/constants.ts";
import { EnvVarList, StringList } from "./spawnEnv/lists.tsx";
import type { CaptureMode, EnvPreview, HarnessConfigStatus, SpawnSettings } from "./types.ts";
import "./SpawnEnvPanel.css";

interface SpawnEnvPanelProps {
	settings: SpawnSettings | null;
	degraded: string | null;
	settingsPath: string;
	onSave: (next: SpawnSettings) => Promise<void>;
	/** Lets the modal refuse to close over unsaved edits — this is the
	 *  only save-required form in Settings; everything else applies
	 *  instantly, so dismissing used to be lossless. */
	onDirtyChange: (dirty: boolean) => void;
}

export const SpawnEnvPanel = ({ settings, degraded, settingsPath, onSave }: SpawnEnvPanelProps) => {
	const [draft, setDraft] = useState<SpawnSettings | null>(settings);
	const [preview, setPreview] = useState<EnvPreview | null>(null);
	const [harnessConfig, setHarnessConfig] = useState<HarnessConfigStatus | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	// Adopt whatever the backend last confirmed. Keyed on the object
	// identity, which only changes on a successful load or save — so
	// this never clobbers half-typed edits.
	useEffect(() => setDraft(settings), [settings]);

	const refreshPreview = useCallback(async () => {
		try {
			const next = await invoke<EnvPreview>("spawn_env_preview");
			setPreview(next);
			return next;
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
			return null;
		}
	}, []);

	useEffect(() => {
		void refreshPreview();
	}, [refreshPreview]);

	// The shipped bundle resolves once at boot and cannot move while
	// the app runs, so this is read once rather than followed.
	useEffect(() => {
		invoke<HarnessConfigStatus>("harness_config_status")
			.then(setHarnessConfig)
			.catch((err: unknown) => {
				setError(err instanceof Error ? err.message : String(err));
			});
	}, []);

	// The probe runs on a helper thread, so a preview taken immediately
	// after a save or re-probe can still read "pending". Follow it until
	// it settles rather than showing a state that is about to be wrong.
	const followProbe = useCallback(async () => {
		for (let attempt = 0; attempt < 12; attempt++) {
			const next = await refreshPreview();
			if (next?.probe.state !== "pending") return;
			await new Promise((resolve) => setTimeout(resolve, 250));
		}
	}, [refreshPreview]);

	const dirty =
		draft !== null && settings !== null && JSON.stringify(draft) !== JSON.stringify(settings);

	const save = async () => {
		if (!draft) return;
		setBusy(true);
		setError(null);
		try {
			// Drop blank rows the user added and never filled in, so an
			// empty box can't become a permanent no-op entry in the file.
			await onSave({
				...draft,
				shell: draft.shell?.trim() ? draft.shell.trim() : null,
				pathPrepend: draft.pathPrepend.map((p) => p.trim()).filter(Boolean),
				extraEnv: draft.extraEnv.filter((v) => v.key.trim()),
			});
			await followProbe();
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		} finally {
			setBusy(false);
		}
	};

	const reprobe = async () => {
		setBusy(true);
		try {
			await invoke("spawn_env_reprobe");
			await followProbe();
		} finally {
			setBusy(false);
		}
	};

	if (!draft) {
		return <div className="sk-help">Loading environment settings…</div>;
	}

	// Windows has no login shell to ask, so the capture controls do
	// nothing there and the shell setting only feeds new Shell harnesses.
	const canProbe = preview?.probe.state !== "not_applicable";

	return (
		<div className="sk-env">
			{degraded && <div className="sk-env-banner sk-env-err">{degraded}</div>}
			{error && <div className="sk-env-banner sk-env-err">{error}</div>}

			<EnvPreviewSection preview={preview} busy={busy} onReprobe={() => void reprobe()} />

			<div className="sk-field">
				<label>Additional directories</label>
				<div className="sk-help">
					Prepended to whatever your shell reported. <code>~</code>, <code>$VAR</code> and{" "}
					<code>%VAR%</code> are expanded; directories that don't exist are skipped rather than
					silently breaking the rest of PATH. Skein never replaces the captured PATH — a mistyped
					replacement is a state you can't recover from inside the app.
				</div>
				<StringList
					items={draft.pathPrepend}
					placeholder="~/tools/bin"
					addLabel="Add directory"
					onChange={(pathPrepend) => setDraft({ ...draft, pathPrepend })}
				/>
			</div>

			<div className="sk-field">
				<label>How to capture</label>
				<select
					className="sk-select"
					value={draft.capture}
					disabled={!canProbe}
					onChange={(e) => setDraft({ ...draft, capture: e.target.value as CaptureMode })}
				>
					{CAPTURE_OPTIONS.map((o) => (
						<option key={o.value} value={o.value}>
							{o.label}
						</option>
					))}
				</select>
				<div className="sk-help">
					{canProbe
						? CAPTURE_OPTIONS.find((o) => o.value === draft.capture)?.desc
						: "Windows has no login shell to ask. Skein re-reads your system and user PATH from the registry on every spawn — so a PATH you just changed takes effect without a reboot — and unions it with the environment Skein was launched with."}
				</div>
			</div>

			<div className="sk-field">
				<label>Shell</label>
				<input
					className="sk-input"
					value={draft.shell ?? ""}
					placeholder="Automatic ($SHELL)"
					spellCheck={false}
					onChange={(e) => setDraft({ ...draft, shell: e.target.value })}
				/>
				<div className="sk-help">
					Absolute path to a shell binary — a path, not a command line.{" "}
					{canProbe
						? "Used both to ask for your PATH and for new Shell harnesses."
						: "Used for new Shell harnesses."}{" "}
					Takes effect for new Shell harnesses and for the Enter-for-shell prompt on any harness
					that has exited; existing harnesses keep the shell they were created with. A path that
					isn't a runnable file is saved but ignored, and flagged above.
				</div>
			</div>

			<div className="sk-field">
				<label>Extra environment variables</label>
				<div className="sk-help">
					Forced into every harness. Useful for things a GUI launch loses — <code>JAVA_HOME</code>,{" "}
					<code>NVM_DIR</code>, a token your rc file sets conditionally.
				</div>
				<EnvVarList
					items={draft.extraEnv}
					onChange={(extraEnv) => setDraft({ ...draft, extraEnv })}
				/>
			</div>

			<EnvToggles
				draft={draft}
				setDraft={setDraft}
				harnessConfig={harnessConfig}
				preview={preview}
			/>

			<div className="sk-env-actions">
				<button
					type="button"
					className="sk-btn primary"
					onClick={() => void save()}
					disabled={!dirty || busy}
				>
					{busy ? "Saving…" : "Save environment"}
				</button>
				<button
					type="button"
					className="sk-btn"
					onClick={() => setDraft(settings)}
					disabled={!dirty || busy}
				>
					Revert
				</button>
				<span className="sk-env-note">
					{dirty ? "Unsaved changes" : "Applies to the next harness you spawn"}
				</span>
			</div>
			<div className="sk-help sk-env-file">{settingsPath}</div>
		</div>
	);
};
