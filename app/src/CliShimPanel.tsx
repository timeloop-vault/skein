// Settings → Command line (epic #255): install the `skein` command, so
// `skein .` in a terminal opens that folder in Skein — VS Code's
// "Install 'code' command in PATH", because a macOS .app's binary is
// never on PATH itself. The script and every decision about it live in
// `cli_shim.rs`; this panel only reports its status and asks for the
// three actions. The command is named per build profile (`skein`,
// `skein-local`, `skein-dev`), so installing one never shadows another.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import "./Settings.css";

/** Mirror of `cli_shim::CliShimStatus`. */
interface CliShimStatus {
	command: string;
	path: string;
	state: "installed" | "absent" | "foreign";
	stale: boolean;
	onPath: boolean | null;
	unsupported: string | null;
}

const errorText = (err: unknown) => (err instanceof Error ? err.message : String(err));

export const CliShimPanel = () => {
	const [status, setStatus] = useState<CliShimStatus | undefined>(undefined);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	useEffect(() => {
		void invoke<CliShimStatus>("cli_shim_status")
			.then(setStatus)
			.catch((err: unknown) => setError(errorText(err)));
	}, []);

	const run = useCallback(async (command: "cli_shim_install" | "cli_shim_uninstall") => {
		setBusy(true);
		setError(null);
		try {
			setStatus(await invoke<CliShimStatus>(command));
		} catch (err: unknown) {
			setError(errorText(err));
		} finally {
			setBusy(false);
		}
	}, []);

	if (!status) return error ? <div className="sk-update-msg err">{error}</div> : null;

	const dir = status.path.slice(0, Math.max(0, status.path.lastIndexOf("/")));
	return (
		<>
			<div className="sk-help">
				<code>{status.command} .</code> in a terminal opens that folder in Skein: it focuses the
				room already there, reopens an archived one, or starts New room with it. Plain{" "}
				<code>{status.command}</code> just brings Skein forward.
			</div>
			{status.unsupported ? (
				<div className="sk-help">{status.unsupported}</div>
			) : (
				<div className="sk-update">
					<div className="sk-update-row">
						<span>
							{status.state === "installed" && (
								<>
									Installed at <code>{status.path}</code>
								</>
							)}
							{status.state === "absent" && "Not installed."}
							{status.state === "foreign" && (
								<>
									<code>{status.path}</code> exists but wasn't installed by Skein, so it's left
									alone.
								</>
							)}
						</span>
						{status.state === "absent" && (
							<button
								type="button"
								className="sk-btn"
								disabled={busy}
								onClick={() => void run("cli_shim_install")}
							>
								Install
							</button>
						)}
						{status.state === "installed" && status.stale && (
							<button
								type="button"
								className="sk-btn"
								disabled={busy}
								onClick={() => void run("cli_shim_install")}
							>
								Update
							</button>
						)}
						{status.state === "installed" && (
							<button
								type="button"
								className="sk-btn ghost"
								disabled={busy}
								onClick={() => void run("cli_shim_uninstall")}
							>
								Remove
							</button>
						)}
					</div>
					{status.state === "installed" && status.stale && (
						<div className="sk-update-msg">
							It points at a different copy of Skein than this one. Update rewrites it.
						</div>
					)}
					{status.state === "installed" && status.onPath === false && (
						<div className="sk-update-msg err">
							<code>{dir}</code> isn't on your shell's PATH, so the command won't be found. Add{" "}
							<code>export PATH="$HOME/.local/bin:$PATH"</code> to your shell profile.
						</div>
					)}
				</div>
			)}
			{error && <div className="sk-update-msg err">{error}</div>}
		</>
	);
};
