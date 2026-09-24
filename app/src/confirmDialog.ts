// A tiny external store backing one in-app confirm dialog (#242),
// replacing the three native `confirm()` calls from
// `@tauri-apps/plugin-dialog` (close room, close a Files harness with
// unsaved buffers, quit with unsaved buffers). Native dialogs don't
// pick up Skein's theme and — on some WebKit builds — silently no-op
// without a host-side handler.
//
// `confirmDialog()` queues a request and returns a promise that
// settles when the user answers. `ConfirmDialogHost` (ConfirmDialog.tsx)
// is the one component that renders the head of the queue; resolving
// it advances to the next request. Requests are FIFO and only the head
// is ever shown — nothing in this app currently stacks two confirms,
// but a future one that does gets a sane order for free instead of two
// overlapping dialogs.
//
// FAIL-OPEN: `confirmDialog()` rejects immediately if no host is
// currently subscribed, rather than queuing a request nobody can ever
// show — that would hang the caller's `await` forever. This matters
// most for the quit path (useAppWindowEffects.ts): its handlers wrap
// the whole confirm sequence in a try/catch that falls back to
// destroying the window on any throw (#196's fail-open requirement) —
// a broken/unmounted dialog host must not make the app unclosable.
// The other two call sites (close room, close harness) just log and do
// nothing on rejection, which is the safe direction for a confirm that
// couldn't be shown.

export interface ConfirmDialogOptions {
	title: string;
	message: string;
	confirmLabel?: string;
	cancelLabel?: string;
	kind?: "warning";
}

export interface ConfirmDialogRequest extends ConfirmDialogOptions {
	id: number;
}

type Listener = () => void;

let nextId = 1;
let queue: ConfirmDialogRequest[] = [];
const listeners = new Set<Listener>();
const pending = new Map<number, (ok: boolean) => void>();

function notify(): void {
	for (const listener of listeners) listener();
}

/** Queue a confirm request; resolves true/false when the host answers it. */
export function confirmDialog(opts: ConfirmDialogOptions): Promise<boolean> {
	// This check only guards a request made while NO host has ever
	// subscribed — it does not protect one already queued if the host
	// then unmounts (its listener leaves `listeners`, but any request
	// already sitting in `pending` would simply never settle). That is
	// safe only because `ConfirmDialogHost` is mounted unconditionally
	// in AppOverlays.tsx for the app's lifetime — a real unmount never
	// happens. Deliberately not rejecting on the last unsubscribe
	// either: dev StrictMode unsubscribes and resubscribes a host on
	// every mount, and that transient gap must not fail a request that
	// arrives (or is already queued) in between.
	if (listeners.size === 0) {
		return Promise.reject(
			new Error("confirmDialog: no ConfirmDialogHost is mounted to show this dialog"),
		);
	}
	return new Promise<boolean>((resolve) => {
		const id = nextId++;
		pending.set(id, resolve);
		queue = [...queue, { id, ...opts }];
		notify();
	});
}

/** The host calls this when the visible (head) request is answered. */
export function resolveConfirmDialog(id: number, ok: boolean): void {
	const resolve = pending.get(id);
	if (!resolve) return;
	pending.delete(id);
	queue = queue.filter((r) => r.id !== id);
	resolve(ok);
	notify();
}

/** useSyncExternalStore subscribe — tracked so `confirmDialog` knows a host exists. */
export function subscribe(listener: Listener): () => void {
	listeners.add(listener);
	return () => listeners.delete(listener);
}

/** useSyncExternalStore snapshot: the head (visible) request, or null. */
export function getSnapshot(): ConfirmDialogRequest | null {
	return queue[0] ?? null;
}
