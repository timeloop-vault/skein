// Decides when PTY output reaches an xterm Terminal (#592).
//
// A hidden terminal (display:none) still parses every chunk it is given,
// and a write that scrolls it can force per-glyph layout. So while hidden
// we buffer, and flush once the stream has been quiet for `idleFlushMs`
// (a busy hidden terminal costs nothing; a waiting one is current within
// that delay), on reveal, or when the buffer outgrows `maxBufferedChars`.
//
// Overflow flushes; it never drops. Cutting the middle out of a TUI stream
// can lose a mode switch (alt-screen, mouse tracking, bracketed paste) and
// leave the screen wrong for good. A flush is handed over in slices so
// xterm's own write queue can time-slice the parse across frames.

export interface GateSink {
	write(data: string, onParsed?: () => void): void;
}

export interface GateTimers {
	setTimeout(fn: () => void, ms: number): unknown;
	clearTimeout(h: unknown): void;
}

export interface OutputGateOptions {
	idleFlushMs?: number;
	maxBufferedChars?: number;
	sliceChars?: number;
	timers?: GateTimers;
}

export interface OutputGate {
	/** Every write goes through here, so order is preserved. */
	write(data: string): void;
	/** Hidden to visible flushes immediately. */
	setHidden(hidden: boolean): void;
	/** Push everything buffered to the sink now. */
	flush(): void;
	/** Flush, then call `fn` once every flushed slice (data buffered while
	 * hidden) is parsed. Visible pass-through writes are not waited on, so a
	 * visible pane under sustained output still refits at once. Synchronous
	 * when nothing is buffered or in flight. */
	whenDrained(fn: () => void): void;
	/** Nothing buffered and no flushed slice still unparsed, so xterm's state
	 * reflects every byte received. Visible pass-through writes don't count. */
	isSettled(): boolean;
	readonly bufferedChars: number;
	dispose(): void;
}

const DEFAULT_IDLE_FLUSH_MS = 300;
const DEFAULT_MAX_BUFFERED_CHARS = 512 * 1024;
const DEFAULT_SLICE_CHARS = 64 * 1024;

const defaultTimers: GateTimers = {
	setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
	clearTimeout: (h) => globalThis.clearTimeout(h as number),
};

function isHighSurrogate(code: number): boolean {
	return code >= 0xd800 && code <= 0xdbff;
}

export function createOutputGate(
	sink: GateSink,
	initiallyHidden: boolean,
	opts: OutputGateOptions = {},
): OutputGate {
	const idleFlushMs = opts.idleFlushMs ?? DEFAULT_IDLE_FLUSH_MS;
	const maxBuffered = opts.maxBufferedChars ?? DEFAULT_MAX_BUFFERED_CHARS;
	const sliceChars = Math.max(2, opts.sliceChars ?? DEFAULT_SLICE_CHARS);
	const timers = opts.timers ?? defaultTimers;

	let hidden = initiallyHidden;
	let disposed = false;
	let chunks: string[] = [];
	let buffered = 0;
	let inFlight = 0;
	let timer: unknown = null;
	let waiters: Array<() => void> = [];

	const clearTimer = () => {
		if (timer !== null) {
			timers.clearTimeout(timer);
			timer = null;
		}
	};

	const onParsed = () => {
		if (disposed) return;
		inFlight -= 1;
		if (inFlight > 0 || waiters.length === 0) return;
		const ready = waiters;
		waiters = [];
		for (const fn of ready) {
			try {
				fn();
			} catch {
				// one throwing waiter must not drop the rest
			}
		}
	};

	// Only flush() slices count as in flight: a visible terminal's
	// pass-through writes must never delay a refit under sustained output.
	const send = (data: string, counted: boolean) => {
		if (!counted) {
			sink.write(data);
			return;
		}
		inFlight += 1;
		sink.write(data, onParsed);
	};

	const flush = () => {
		if (disposed) return;
		clearTimer();
		if (buffered === 0) return;
		const all = chunks.join("");
		chunks = [];
		buffered = 0;
		let start = 0;
		while (start < all.length) {
			let end = Math.min(start + sliceChars, all.length);
			if (end < all.length && isHighSurrogate(all.charCodeAt(end - 1))) {
				end -= 1;
			}
			send(all.slice(start, end), true);
			start = end;
		}
	};

	return {
		write(data) {
			if (disposed || data.length === 0) return;
			if (!hidden) {
				send(data, false);
				return;
			}
			chunks.push(data);
			buffered += data.length;
			if (buffered > maxBuffered) {
				flush();
				return;
			}
			clearTimer();
			timer = timers.setTimeout(() => {
				timer = null;
				flush();
			}, idleFlushMs);
		},
		setHidden(next) {
			if (disposed) return;
			hidden = next;
			if (!next) flush();
		},
		flush,
		whenDrained(fn) {
			if (disposed) return;
			flush();
			if (inFlight === 0) {
				fn();
				return;
			}
			waiters.push(fn);
		},
		isSettled() {
			return buffered === 0 && inFlight === 0;
		},
		get bufferedChars() {
			return buffered;
		},
		dispose() {
			if (disposed) return;
			clearTimer();
			disposed = true;
			chunks = [];
			buffered = 0;
			waiters = [];
		},
	};
}
