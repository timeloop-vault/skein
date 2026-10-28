import { invoke } from "@tauri-apps/api/core";
import type { OpencodeVersion } from "./types.ts";

/// The installed opencode CLI version (`opencode --version`), or null when
/// the probe fails or can't tell (#675: unknown is treated as v1).
export async function opencodeCliVersion(): Promise<OpencodeVersion | null> {
	try {
		return await invoke<OpencodeVersion | null>("opencode_cli_version");
	} catch {
		return null;
	}
}
