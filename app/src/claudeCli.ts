import { invoke } from "@tauri-apps/api/core";

/// The installed Claude Code version (`claude --version`), or null when
/// it can't be determined (#491).
export async function installedClaudeVersion(): Promise<string | null> {
	try {
		return await invoke<string | null>("claude_cli_version");
	} catch {
		return null;
	}
}
