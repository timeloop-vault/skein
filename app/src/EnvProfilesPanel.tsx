// Settings → Shell & environment: named env profiles (#615). Edits the
// parent's draft; saving is the parent's "Save environment" button.

import {
	addProfile,
	deleteProfile,
	profileNames,
	renameProfile,
	validateProfiles,
	withActiveProfile,
} from "./envProfiles.ts";
import type { EnvProfile, EnvProfileVar, SpawnSettings } from "./types.ts";
import "./EnvProfilesPanel.css";

interface Props {
	draft: SpawnSettings;
	setDraft: (next: SpawnSettings) => void;
}

const VarRow = ({
	v,
	onChange,
	onRemove,
}: {
	v: EnvProfileVar;
	onChange: (next: EnvProfileVar) => void;
	onRemove: () => void;
}) => (
	<div className="sk-list-row">
		<input
			className="sk-input sk-env-key"
			value={v.key}
			placeholder="NAME"
			spellCheck={false}
			onChange={(e) => onChange({ ...v, key: e.target.value })}
		/>
		<input
			className="sk-input"
			value={v.unset ? "" : v.value}
			placeholder={v.unset ? "(removed)" : "value"}
			disabled={v.unset}
			spellCheck={false}
			onChange={(e) => onChange({ ...v, value: e.target.value })}
		/>
		<label className="sk-envp-unset" title="Remove this variable from the environment">
			<input
				type="checkbox"
				checked={v.unset}
				onChange={(e) => onChange({ ...v, unset: e.target.checked })}
			/>
			unset
		</label>
		<button type="button" className="sk-btn sk-list-btn" title="Remove" onClick={onRemove}>
			✕
		</button>
	</div>
);

export const EnvProfilesPanel = ({ draft, setDraft }: Props) => {
	const profiles = draft.envProfiles ?? [];
	const problem = validateProfiles(profiles);

	const patch = (i: number, next: EnvProfile) =>
		setDraft({ ...draft, envProfiles: profiles.map((p, j) => (j === i ? next : p)) });

	return (
		<div className="sk-field sk-envp">
			<label>Environment profiles</label>
			<div className="sk-help">
				Named sets of variables to set or unset, layered on top of the extra variables above. One
				profile is active at a time. Applies on the next start of a harness; running harnesses keep
				their environment. Editing a profile's variables works the same way: restart a harness to
				pick up changes (the restart marker only tracks which profile, not its contents). On macOS
				and Linux variable names are case-sensitive, so a proxy profile typically lists both{" "}
				<code>HTTP_PROXY</code> and <code>http_proxy</code> (same for <code>HTTPS_PROXY</code> and{" "}
				<code>NO_PROXY</code>). Loopback is always kept in <code>NO_PROXY</code>.
			</div>
			<div className="sk-envp-active">
				<span>Active profile</span>
				<select
					className="sk-select"
					value={draft.activeEnvProfile ?? ""}
					onChange={(e) => setDraft(withActiveProfile(draft, e.target.value || null))}
				>
					<option value="">None</option>
					{profileNames(draft).map((n, i) => (
						<option key={i} value={n}>
							{n}
						</option>
					))}
				</select>
			</div>
			{profiles.map((p, i) => (
				// Index keys: the name is user-editable, so not a stable identity.
				<div className="sk-envp-profile" key={i}>
					<div className="sk-list-row">
						<input
							className="sk-input"
							value={p.name}
							placeholder="Profile name"
							spellCheck={false}
							onChange={(e) => setDraft(renameProfile(draft, i, e.target.value))}
						/>
						<button
							type="button"
							className="sk-btn"
							onClick={() => setDraft(deleteProfile(draft, i))}
						>
							Delete profile
						</button>
					</div>
					{p.vars.map((v, j) => (
						<VarRow
							key={j}
							v={v}
							onChange={(next) =>
								patch(i, { ...p, vars: p.vars.map((x, k) => (k === j ? next : x)) })
							}
							onRemove={() => patch(i, { ...p, vars: p.vars.filter((_, k) => k !== j) })}
						/>
					))}
					<button
						type="button"
						className="sk-btn sk-list-add"
						onClick={() =>
							patch(i, { ...p, vars: [...p.vars, { key: "", value: "", unset: false }] })
						}
					>
						Add variable
					</button>
				</div>
			))}
			{problem && <div className="sk-env-banner sk-env-err">{problem}</div>}
			<button
				type="button"
				className="sk-btn sk-list-add"
				onClick={() => setDraft(addProfile(draft))}
			>
				Add profile
			</button>
		</div>
	);
};
