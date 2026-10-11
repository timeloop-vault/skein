// Updater proxy (#614): DTO mirrors of the Rust updater commands, invoke
// wrappers, and the pure wording used by Settings → About.

import { invoke } from "@tauri-apps/api/core";

export interface EffectiveProxyDto {
	/** "setting" | "env:HTTPS_PROXY" | "env:https_proxy" | … | "system" */
	source: string;
	url: string | null;
	noProxy: string | null;
	bypassedByNoProxy: boolean;
	host: string | null;
	skipped: string[];
}

export interface UpdaterProxyDto {
	/** Redacted: no credentials. Never write it back. */
	setting: string | null;
	loadWarning: string | null;
	effective: EffectiveProxyDto;
}

export interface UpdaterDiagnosisDto {
	url: string;
	effective: EffectiveProxyDto;
	ok: boolean;
	status: number | null;
	chain: string[];
}

export const RELEASES_URL = "https://github.com/timeloop-vault/skein/releases/latest";

export const updaterProxyGet = (): Promise<UpdaterProxyDto> => invoke("updater_proxy_get");

export const updaterProxySet = (proxy: string | null): Promise<UpdaterProxyDto> =>
	invoke("updater_proxy_set", { proxy });

/** Resolved proxy URL, credentials included: pass it to check(), never render or log it. */
export const updaterCheckProxy = (): Promise<string | null> => invoke("updater_check_proxy");

export const updaterDiagnose = (): Promise<UpdaterDiagnosisDto> => invoke("updater_diagnose");

export const describeEffective = (e: EffectiveProxyDto): string => {
	if (e.url == null) {
		return "No proxy set: using the system's manual proxy settings, if any (PAC files are not evaluated)";
	}
	const from =
		e.source === "setting"
			? "Settings"
			: e.source.startsWith("env:")
				? `$${e.source.slice(4)}`
				: e.source;
	const base = `Using ${e.url} from ${from}`;
	let out = e.bypassedByNoProxy
		? `${base}, but skipped for ${e.host ?? "the update server"} because of NO_PROXY`
		: base;
	if (e.skipped.length > 0) out += ` (ignored invalid: ${e.skipped.join(", ")})`;
	return out;
};

export const formatFailure = (message: string, d: UpdaterDiagnosisDto | null): string[] => {
	const lines = [`Update failed: ${message}`];
	if (!d) return lines;
	if (d.chain.length > 0) lines.push(`Cause: ${d.chain.join(" → ")}`);
	lines.push(describeEffective(d.effective));
	return lines;
};
