/// `pty_spawn`'s resolved value. `injected` mirrors whether #215's
/// config injection was non-empty for this spawn — see
/// `harnessActivity.injected` and the #238 nudge gate in
/// `harnessInput.ts`, the only consumers.
export interface PtySpawnResult {
	id: string;
	injected: boolean;
	/// `CLAUDE_CODE_DISABLE_MOUSE_CLICKS` is truthy in the child's env
	/// (#401): Skein must open clicked links itself.
	mouseClicksDisabled: boolean;
}
