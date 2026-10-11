// Settings → About: the updater's HTTP proxy (#614).

import { useEffect, useState } from "react";
import {
	describeEffective,
	type UpdaterProxyDto,
	updaterProxyGet,
	updaterProxySet,
} from "./updaterProxy.ts";

const errText = (err: unknown): string => (err instanceof Error ? err.message : String(err));

export const UpdaterProxyField = () => {
	const [dto, setDto] = useState<UpdaterProxyDto | undefined>(undefined);
	const [text, setText] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | undefined>(undefined);

	useEffect(() => {
		void updaterProxyGet()
			.then((d) => {
				setDto(d);
			})
			.catch((err: unknown) => setError(errText(err)));
	}, []);

	const save = (value: string | null) => {
		setError(undefined);
		setBusy(true);
		void updaterProxySet(value)
			.then((d) => {
				setDto(d);
				setText("");
			})
			.catch((err: unknown) => setError(errText(err)))
			.finally(() => setBusy(false));
	};

	return (
		<div className="sk-update-row" style={{ flexDirection: "column", alignItems: "stretch" }}>
			<label htmlFor="sk-updater-proxy">Update proxy</label>
			<div className="sk-update-row">
				<input
					id="sk-updater-proxy"
					className="sk-input"
					value={text}
					placeholder="host:port or http://host:port"
					spellCheck={false}
					onChange={(e) => setText(e.target.value)}
				/>
				<button
					type="button"
					className="sk-btn"
					disabled={text.trim() === "" || busy}
					onClick={() => save(text.trim())}
				>
					Save
				</button>
				<button
					type="button"
					className="sk-btn"
					disabled={dto?.setting == null || busy}
					onClick={() => save(null)}
				>
					Clear
				</button>
			</div>
			<div className="sk-update-msg">Current: {dto?.setting ?? "none"}</div>
			{dto?.loadWarning && (
				<div className="sk-update-msg err">
					updater.json could not be read and was ignored: {dto.loadWarning}
				</div>
			)}
			{error && <div className="sk-update-msg err">{error}</div>}
			{dto && <div className="sk-update-msg">{describeEffective(dto.effective)}</div>}
		</div>
	);
};
