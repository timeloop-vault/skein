// Settings → Nudges → "Context limit" (#689). Unlike the registry nudges this
// one is sent by the daemon, so it lives in settings.json
// (`SpawnSettings.contextNudge`), saved like `contextThreshold`.

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { bodyToStore, effectiveBody, isCustomBody } from "./contextNudge.ts";
import { getContextNudge, useContextNudge } from "./contextNudgeStore.ts";
import { useContextThreshold } from "./contextThresholdStore.ts";
import { queueSettingsWrite } from "./envProfileStore.ts";
import type { SpawnSettings, SpawnSettingsPayload } from "./types.ts";
import "./ContextNudgeRow.css";

export const ContextNudgeRow = ({
	onSpawnSettings,
}: {
	onSpawnSettings: (next: SpawnSettings) => Promise<void>;
}) => {
	const stored = useContextNudge();
	const limit = useContextThreshold();
	const resolved = effectiveBody(stored.body);
	const [draft, setDraft] = useState(resolved);
	const [error, setError] = useState<string | null>(null);
	const draftRef = useRef(draft);
	const resolvedRef = useRef(resolved);

	// Follow the saved text when it changes from outside this row's typing
	// (Reset, another window); it never changes mid-keystroke.
	useEffect(() => {
		setDraft(resolved);
		draftRef.current = resolved;
		resolvedRef.current = resolved;
	}, [resolved]);

	// Re-read the settings inside the shared write chain, so only the nudge
	// changes and a Shell & environment save that landed first is kept.
	const save = async (patch: { enabled?: boolean; body?: string | null }) => {
		setError(null);
		try {
			await queueSettingsWrite(async () => {
				const current = await invoke<SpawnSettingsPayload>("spawn_settings_load");
				const base = current.settings.contextNudge ?? getContextNudge();
				await onSpawnSettings({
					...current.settings,
					contextNudge: {
						enabled: patch.enabled ?? base.enabled,
						body: patch.body === undefined ? base.body : patch.body,
					},
				});
			});
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		}
	};

	const commit = () => {
		const next = bodyToStore(draft);
		if (next === stored.body) {
			if (next === null) setDraft(resolved);
			return;
		}
		void save({ body: next });
	};

	// Escape closes Settings without a blur; flush the draft on unmount.
	// biome-ignore lint/correctness/useExhaustiveDependencies: mount-once flush reading refs
	useEffect(() => {
		return () => {
			if (draftRef.current !== resolvedRef.current) {
				const next = bodyToStore(draftRef.current);
				if (next !== getContextNudge().body) void save({ body: next });
			}
		};
	}, []);

	return (
		<div className="sk-nudge">
			<div className="sk-nudge-head">
				<label className="sk-nudge-label sk-ctxnudge-switch">
					<input
						type="checkbox"
						checked={stored.enabled}
						onChange={(e) => void save({ enabled: e.target.checked })}
					/>
					Context limit
				</label>
				{isCustomBody(stored.body) && <span className="sk-nudge-edited">edited</span>}
				<button
					type="button"
					className="sk-btn ghost sk-nudge-reset"
					disabled={stored.body === null}
					onClick={() => void save({ body: null })}
				>
					Reset to default
				</button>
			</div>
			<div className="sk-help">
				Sent by Skein once when a harness's context passes the context limit. {"{tokens}"} and{" "}
				{"{limit}"} are replaced with the numbers.
			</div>
			{limit === null && (
				<div className="sk-help sk-ctxnudge-muted">
					Never fires until a context limit is set: Settings, Agents, "Mark harness context above".
				</div>
			)}
			<textarea
				className="sk-textarea sk-nudge-body"
				spellCheck={false}
				rows={4}
				value={draft}
				disabled={!stored.enabled}
				onChange={(e) => {
					setDraft(e.target.value);
					draftRef.current = e.target.value;
				}}
				onBlur={commit}
			/>
			{error && <div className="sk-default-agent-note err">{error}</div>}
		</div>
	);
};
