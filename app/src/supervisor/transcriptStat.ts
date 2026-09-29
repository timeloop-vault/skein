import { invoke } from "@tauri-apps/api/core";

export type TranscriptStat = { size: number; mtimeMs: number };

/** Size and mtime of a Claude harness's main transcript, or `null` when
 *  the file doesn't exist (#423). */
export async function claudeTranscriptStat(
	sessionId: string,
	cwd: string,
): Promise<TranscriptStat | null> {
	return invoke<TranscriptStat | null>("claude_transcript_stat", {
		sessionId,
		cwd,
	});
}
