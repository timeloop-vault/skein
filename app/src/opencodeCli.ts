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

/// #675: record the CLI version on a harness at spawn. Fire-and-forget (the
/// caller never awaits it on the spawn path); a result that lands after
/// `isCancelled()` is dropped.
export function recordOpencodeVersion(
	kind: string,
	record: ((version: OpencodeVersion | null) => void) | undefined,
	isCancelled: () => boolean,
): void {
	if (kind !== "opencode" || !record) return;
	void opencodeCliVersion().then((v) => {
		if (!isCancelled()) record(v);
	});
}
