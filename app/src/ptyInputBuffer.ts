// Holds xterm's `onData` output for the window between creating the
// terminal channel and `pty_spawn` resolving, when the real input
// listener cannot be attached yet (#484).
//
// Under portable-pty 0.9 ConPTY inherits the cursor: conhost sends
// `ESC[6n` and withholds all child output until the host replies. xterm
// answers through `onData`; with no listener that reply is lost and the
// terminal stays blank forever.

export interface OnDataSource {
	onData(cb: (data: string) => void): { dispose(): void };
}

export interface PtyInputBuffer {
	/** Stop collecting and return everything collected, in order. */
	takeAndDispose(): string[];
	/** Stop collecting and drop whatever was collected. */
	dispose(): void;
}

export function bufferPtyInput(term: OnDataSource): PtyInputBuffer {
	let chunks: string[] = [];
	const sub = term.onData((data) => {
		chunks.push(data);
	});
	return {
		takeAndDispose() {
			sub.dispose();
			const out = chunks;
			chunks = [];
			return out;
		},
		dispose() {
			sub.dispose();
			chunks = [];
		},
	};
}
