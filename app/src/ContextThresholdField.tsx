// Settings → the context-size threshold (#368). Stored in settings.json
// (`SpawnSettings.contextThreshold`); null = off.

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { parseTokenCount, thresholdDisplay, thresholdTextChanged } from "./contextSize.ts";
import { queueSettingsWrite } from "./envProfileStore.ts";
import type { SpawnSettings, SpawnSettingsPayload } from "./types.ts";

const show = thresholdDisplay;

export const ContextThresholdField = ({
	value,
	loaded,
	onSpawnSettings,
}: {
	value: number | null | undefined;
	/** False while the first settings load is in flight. */
	loaded: boolean;
	onSpawnSettings: (next: SpawnSettings) => Promise<void>;
}) => {
	const [text, setText] = useState(show(value));
	const [invalid, setInvalid] = useState(false);
	const [error, setError] = useState<string | null>(null);

	useEffect(() => {
		setText(show(value));
		setInvalid(false);
	}, [value]);

	const commit = async () => {
		if (!thresholdTextChanged(text, value)) {
			setInvalid(false);
			return;
		}
		const parsed = parseTokenCount(text);
		if (parsed === "invalid") {
			setInvalid(true);
			return;
		}
		setInvalid(false);
		if (parsed === (value ?? null)) {
			setText(show(value));
			return;
		}
		setError(null);
		try {
			// Re-read inside the shared write chain, so a Shell & environment
			// save (or a profile switch) that landed first is not overwritten
			// by whatever settings object this component last rendered with.
			await queueSettingsWrite(async () => {
				const current = await invoke<SpawnSettingsPayload>("spawn_settings_load");
				await onSpawnSettings({ ...current.settings, contextThreshold: parsed });
			});
		} catch (err) {
			setError(err instanceof Error ? err.message : String(err));
		}
	};

	return (
		<div className="sk-field">
			<label htmlFor="sk-context-threshold">Mark harness context above</label>
			<div className="sk-help">
				Marks a harness tab (and its room tab) amber when the harness's latest-turn context is above
				this; nothing else happens. Empty is off.
			</div>
			<input
				id="sk-context-threshold"
				className="sk-input"
				placeholder="e.g. 150k"
				disabled={!loaded}
				value={text}
				onChange={(e) => {
					setText(e.target.value);
					setInvalid(false);
				}}
				onBlur={() => void commit()}
				onKeyDown={(e) => {
					if (e.key === "Enter") void commit();
				}}
			/>
			{invalid && (
				<div className="sk-default-agent-note err">
					Enter a count above 0, like 150000, 150k or 1.2M (empty turns it off).
				</div>
			)}
			{error && <div className="sk-default-agent-note err">{error}</div>}
		</div>
	);
};
