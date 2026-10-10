// Settings → About: version, the in-app updater and the agent review API
// status. Tauri's updater plugin handles the cryptographic verification
// (against the pubkey in tauri.conf.json) and writes the new bundle in place.

import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { check } from "@tauri-apps/plugin-updater";
import { useCallback, useEffect, useState } from "react";
import { flushRoomsBounded } from "./roomsFlush.ts";
import "./Settings.css";

type UpdateState =
	| { status: "idle" }
	| { status: "checking" }
	| { status: "current" }
	| { status: "available"; version: string; notes: string | undefined }
	| { status: "downloading"; downloaded: number; total: number | undefined }
	| { status: "ready" }
	| { status: "error"; message: string };

/// Mirror of `agent_api::state::AgentApiStatus` (#213).
interface AgentApiStatus {
	/** The bound port, or null when the listener never came up. */
	port: number | null;
	error: string | null;
}

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

	const [update, setUpdate] = useState<UpdateState>({ status: "idle" });
	const checkForUpdate = useCallback(async () => {
		setUpdate({ status: "checking" });
		try {
			const found = await check();
			if (!found) {
				setUpdate({ status: "current" });
				return;
			}
			setUpdate({ status: "available", version: found.version, notes: found.body });
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : String(err);
			setUpdate({ status: "error", message });
		}
	}, []);

	const downloadAndInstall = useCallback(async () => {
		setUpdate({ status: "downloading", downloaded: 0, total: undefined });
		try {
			const found = await check();
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
					setUpdate({ status: "ready" });
				}
			});
		} catch (err: unknown) {
			const message = err instanceof Error ? err.message : String(err);
			setUpdate({ status: "error", message });
		}
	}, []);

	return (
		<div className="sk-field">
			<label>About</label>
			<div className="sk-update">
				<div className="sk-update-row">
					<span className="sk-update-version">Skein {version || "—"}</span>
					<button
						type="button"
						className="sk-btn"
						onClick={checkForUpdate}
						disabled={update.status === "checking" || update.status === "downloading"}
					>
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
				{update.status === "ready" && (
					<div className="sk-update-msg ok">
						Update installed. Restart Skein to use the new version.
					</div>
				)}
				{update.status === "error" && (
					<div className="sk-update-msg err">Update failed: {update.message}</div>
				)}
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
