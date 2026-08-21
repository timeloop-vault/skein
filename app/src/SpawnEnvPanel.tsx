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
import type { CaptureMode, DropReason, EnvPreview, EnvVar, SpawnSettings } from "./types.ts";

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

const CAPTURE_OPTIONS: { value: CaptureMode; label: string; desc: string }[] = [
	{
		value: "login-interactive",
		label: "Login + interactive shell",
		desc: "Sources your whole startup chain (.zshenv, .zprofile, .zshrc). Closest to what a new terminal window gives you — interactive-only files are where version managers and package managers install themselves.",
	},
	{
		value: "login",
		label: "Login files only",
		desc: "Skips .zshrc and friends. Faster, and avoids prompt frameworks and completion loading. Use this if your interactive config is slow or fragile.",
	},
	{
		value: "none",
		label: "Don't ask a shell",
		desc: "Use the environment Skein itself was launched with, plus your additions below. Honest choice if you always start Skein from a terminal — and the escape hatch if a probe misbehaves.",
	},
];

const PROBE_TONE: Record<string, "ok" | "warn" | "err" | "muted"> = {
	captured: "ok",
	pending: "muted",
	// A deliberate setting, not a failure — see ProbeFailure::Disabled.
	disabled: "muted",
	not_applicable: "muted",
	unsupported_shell: "warn",
	timeout: "err",
	spawn_failed: "err",
	no_payload: "warn",
};

const DROP_REASON: Record<DropReason, string> = {
	unresolved: "unset variable",
	not_absolute: "not an absolute path",
	separator: "contains a path separator",
	missing: "directory not found",
	duplicate: "already on PATH",
};

const SOURCE_LABEL: Record<string, string> = {
	added: "yours",
	shell: "shell",
	inherited: "inherited",
};

/** A list of freeform strings with add / remove / reorder. */
const StringList = ({
	items,
	placeholder,
	addLabel,
	onChange,
}: {
	items: string[];
	placeholder: string;
	addLabel: string;
	onChange: (next: string[]) => void;
}) => {
	const replace = (i: number, value: string) =>
		onChange(items.map((v, j) => (j === i ? value : v)));
	const move = (i: number, delta: number) => {
		const j = i + delta;
		if (j < 0 || j >= items.length) return;
		const next = [...items];
		const [row] = next.splice(i, 1);
		if (row === undefined) return;
		next.splice(j, 0, row);
		onChange(next);
	};
	return (
		<div className="sk-list">
			{items.map((item, i) => (
				// Index keys are correct here: rows are positional, the
				// list is short, and the value itself is user-editable
				// (so it is not a stable identity).
				<div className="sk-list-row" key={i}>
					<input
						className="sk-input"
						value={item}
						placeholder={placeholder}
						spellCheck={false}
						onChange={(e) => replace(i, e.target.value)}
					/>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Move up"
						disabled={i === 0}
						onClick={() => move(i, -1)}
					>
						↑
					</button>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Move down"
						disabled={i === items.length - 1}
						onClick={() => move(i, 1)}
					>
						↓
					</button>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Remove"
						onClick={() => onChange(items.filter((_, j) => j !== i))}
					>
						✕
					</button>
				</div>
			))}
			<button type="button" className="sk-btn sk-list-add" onClick={() => onChange([...items, ""])}>
				{addLabel}
			</button>
		</div>
	);
};

const EnvVarList = ({
	items,
	onChange,
}: {
	items: EnvVar[];
	onChange: (next: EnvVar[]) => void;
}) => (
	<div className="sk-list">
		{items.map((item, i) => (
			<div className="sk-list-row" key={i}>
				<input
					className="sk-input sk-env-key"
					value={item.key}
					placeholder="NAME"
					spellCheck={false}
					onChange={(e) =>
						onChange(items.map((v, j) => (j === i ? { ...v, key: e.target.value } : v)))
					}
				/>
				<input
					className="sk-input"
					value={item.value}
					placeholder="value"
					spellCheck={false}
					onChange={(e) =>
						onChange(items.map((v, j) => (j === i ? { ...v, value: e.target.value } : v)))
					}
				/>
				<button
					type="button"
					className="sk-btn sk-list-btn"
					title="Remove"
					onClick={() => onChange(items.filter((_, j) => j !== i))}
				>
					✕
				</button>
			</div>
		))}
		<button
			type="button"
			className="sk-btn sk-list-add"
			onClick={() => onChange([...items, { key: "", value: "" }])}
		>
			Add variable
		</button>
	</div>
);

export const SpawnEnvPanel = ({ settings, degraded, settingsPath, onSave }: SpawnEnvPanelProps) => {
	const [draft, setDraft] = useState<SpawnSettings | null>(settings);
	const [preview, setPreview] = useState<EnvPreview | null>(null);
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

	const probeTone = preview ? (PROBE_TONE[preview.probe.state] ?? "muted") : "muted";
	const missingPrograms = preview?.programs.filter((p) => p.resolved === null) ?? [];
	// Windows has no login shell to ask, so the capture controls do
	// nothing there and the shell setting only feeds new Shell harnesses.
	const canProbe = preview?.probe.state !== "not_applicable";

	return (
		<div className="sk-env">
			{degraded && <div className="sk-env-banner sk-env-err">{degraded}</div>}
			{error && <div className="sk-env-banner sk-env-err">{error}</div>}

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
				<button type="button" className="sk-btn" onClick={() => void reprobe()} disabled={busy}>
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
