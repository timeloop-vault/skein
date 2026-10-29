// Settings → About: version, the in-app updater and the agent review API
// status. Tauri's updater plugin handles the cryptographic verification
// (against the pubkey in tauri.conf.json) and writes the new bundle in place.

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { check } from "@tauri-apps/plugin-updater";
import { useCallback, useEffect, useState } from "react";
import { restartApp } from "./exitGuard.ts";
import { flushPerfSummary } from "./perfTelemetry.ts";
import { flushRoomsBounded } from "./roomsFlush.ts";
import { UpdaterProxyField } from "./UpdaterProxyField.tsx";
import { formatFailure, RELEASES_URL, updaterCheckProxy, updaterDiagnose } from "./updaterProxy.ts";
import "./Settings.css";

type UpdateState =
	| { status: "idle" }
	| { status: "checking" }
	| { status: "current" }
	| { status: "available"; version: string; notes: string | undefined }
	| { status: "downloading"; downloaded: number; total: number | undefined }
	| { status: "installing" }
	| { status: "ready" }
	| { status: "restarting" }
	| { status: "error"; message: string; lines: string[] };

// #692: an install outlives the About panel. Closing Settings must not
// lose the restart, and a fresh check would offer the same version again
// while the old binary is still the one running.
let installedThisSession = false;

/// Mirror of `agent_api::state::AgentApiStatus` (#213).
interface AgentApiStatus {
	/** The bound port, or null when the listener never came up. */
	port: number | null;
	error: string | null;
}

// #614: the plugin can't be hooked globally, so resolve the proxy per check.
// The URL may hold credentials: it is never rendered or logged.
const checkWithProxy = async () => {
	const proxy = await updaterCheckProxy().catch(() => {
		console.warn("[skein] updater_check_proxy failed; checking without it");
		return null;
	});
	return check(proxy ? { proxy } : undefined);
};

export const AboutSettings = () => {
	// Updater UI: pulled into the modal so the user can check + install
	// updates from a discoverable surface. Tauri's updater plugin
	// handles the cryptographic verification (against the pubkey in
	// tauri.conf.json) and writes the new bundle in place.
	const [version, setVersion] = useState<string>("");
	useEffect(() => {
		void getVersion()
			.then(setVersion)
			.catch((err: unknown) => {
				// #505: a failed lookup must not be an unhandled rejection.
				console.warn("[skein] getVersion failed:", err);
				setVersion("unknown");
			});
	}, []);

	// #213: whether the agent-facing review API came up. Shown because
	// the alternative is an agent whose review tools silently do not
	// exist, which looks like a broken harness rather than a bound port
	// that never bound (#176).
	const [agentApi, setAgentApi] = useState<AgentApiStatus | undefined>(undefined);
	useEffect(() => {
		void invoke<AgentApiStatus>("agent_api_status")
			.then(setAgentApi)
			.catch((err: unknown) => {
				setAgentApi({ port: null, error: err instanceof Error ? err.message : String(err) });
			});
	}, []);

	const [update, setUpdate] = useState<UpdateState>(() =>
		installedThisSession ? { status: "ready" } : { status: "idle" },
	);
	// #614: keep the plugin's message, then ask Rust why (cause chain +
	// effective proxy). The diagnosis failing must not hide the real error.
	const failWith = useCallback(async (err: unknown) => {
		const message = err instanceof Error ? err.message : String(err);
		const diagnosis = await updaterDiagnose().catch((e: unknown) => {
			console.warn("[skein] updater_diagnose failed:", e);
			return null;
		});
		setUpdate({ status: "error", message, lines: formatFailure(message, diagnosis) });
	}, []);

	const checkForUpdate = useCallback(async () => {
		setUpdate({ status: "checking" });
		try {
			const found = await checkWithProxy();
			if (!found) {
				setUpdate({ status: "current" });
				return;
			}
			setUpdate({ status: "available", version: found.version, notes: found.body });
		} catch (err: unknown) {
			await failWith(err);
		}
	}, [failWith]);

	const downloadAndInstall = useCallback(async () => {
		setUpdate({ status: "downloading", downloaded: 0, total: undefined });
		try {
			const found = await checkWithProxy();
			if (!found) {
				setUpdate({ status: "current" });
				return;
			}
			// #594: the installer can terminate the app; land the rooms first.
			await flushRoomsBounded();
			let downloaded = 0;
			let total: number | undefined;
			await found.downloadAndInstall((event) => {
				if (event.event === "Started") {
					total = event.data.contentLength;
					setUpdate({ status: "downloading", downloaded: 0, total });
				} else if (event.event === "Progress") {
					downloaded += event.data.chunkLength;
					setUpdate({ status: "downloading", downloaded, total });
				} else if (event.event === "Finished") {
					// Download is done; the install (which may end the app) starts now.
					flushPerfSummary("shutdown");
					setUpdate({ status: "installing" });
				}
			});
			// Only now is the bundle replaced: restarting any earlier could
			// relaunch a half-written one.
			installedThisSession = true;
			setUpdate({ status: "ready" });
		} catch (err: unknown) {
			await failWith(err);
		}
	}, [failWith]);

	const restart = useCallback(async () => {
		setUpdate({ status: "restarting" });
		try {
			if (!(await restartApp())) setUpdate({ status: "ready" });
		} catch (err: unknown) {
			console.error("[skein] restart failed:", err);
			setUpdate({ status: "ready" });
		}
	}, []);

	// Nothing to check while an update is in flight or waiting on the restart.
	const busy = ["checking", "downloading", "installing", "ready", "restarting"].includes(
		update.status,
	);

	return (
		<div className="sk-field">
			<label>About</label>
			<div className="sk-update">
				<div className="sk-update-row">
					<span className="sk-update-version">Skein {version || "—"}</span>
					<button type="button" className="sk-btn" onClick={checkForUpdate} disabled={busy}>
						{update.status === "checking" ? "Checking…" : "Check for updates"}
					</button>
				</div>
				{update.status === "current" && (
					<div className="sk-update-msg ok">You're on the latest version.</div>
				)}
				{update.status === "available" && (
					<div className="sk-update-msg">
						<div>
							Update available: <strong>{update.version}</strong>
						</div>
						{update.notes && <pre className="sk-update-notes">{update.notes}</pre>}
						<button
							type="button"
							className="sk-btn primary"
							onClick={downloadAndInstall}
							style={{ marginTop: 6 }}
						>
							Download & install
						</button>
					</div>
				)}
				{update.status === "downloading" && (
					<div className="sk-update-msg">
						Downloading…{" "}
						{update.total
							? `${Math.round((update.downloaded / update.total) * 100)}%`
							: `${(update.downloaded / 1024).toFixed(0)} KB`}
					</div>
				)}
				{update.status === "installing" && <div className="sk-update-msg">Installing…</div>}
				{(update.status === "ready" || update.status === "restarting") && (
					<div className="sk-update-msg ok">
						<div>Update installed. Restart Skein to use the new version.</div>
						<button
							type="button"
							className="sk-btn primary"
							onClick={restart}
							disabled={update.status === "restarting"}
							style={{ marginTop: 6 }}
						>
							{update.status === "restarting" ? "Restarting…" : "Restart now"}
						</button>
					</div>
				)}
				{update.status === "error" && (
					<div className="sk-update-msg err">
						{update.lines.map((line) => (
							<div key={line}>{line}</div>
						))}
						<button
							type="button"
							className="sk-btn"
							style={{ marginTop: 6 }}
							onClick={() => void openUrl(RELEASES_URL)}
						>
							Download manually
						</button>
					</div>
				)}
				<UpdaterProxyField />
				{agentApi &&
					(agentApi.port != null ? (
						<div className="sk-update-msg">
							Agent review API on 127.0.0.1:{agentApi.port} — harnesses get{" "}
							<code>SKEIN_REVIEW_URL</code> and <code>SKEIN_REVIEW_TOKEN</code>.
						</div>
					) : (
						<div className="sk-update-msg err">
							Agent review API is down: {agentApi.error ?? "unknown reason"}. Agents in this session
							cannot read review comments.
						</div>
					))}
			</div>
		</div>
	);
};
