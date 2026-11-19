// Who writes the pane notices that follow a PTY (#809, T-5). In raw mode the
// view writes them into its own widget (the exit banner, the opencode server
// stopped line). In frames mode the daemon appends them to its engine and every
// viewer gets them in a frame, so a local write would draw them twice.
//
// The mode is learned from the channel itself: a frames-mode daemon sends a
// `grid` event first on every attach, before any `exit`, so this is known by
// the time an exit arrives, which the ensure call's resolved `terminalMode` is
// not (an already-exited process delivers its `exit` inside that call). Slice
// T-6 deletes this with the raw path.

export interface NoticeGate {
	/** Feed every channel event through it. */
	observe(ev: { kind: string }): void;
	/** Write a notice locally: dropped in frames mode, where the daemon wrote it. */
	write(s: string): void;
}

export function createNoticeGate(write: (s: string) => void): NoticeGate {
	let frames = false;
	return {
		observe(ev) {
			if (ev.kind === "grid") frames = true;
		},
		write(s) {
			if (!frames) write(s);
		},
	};
}
