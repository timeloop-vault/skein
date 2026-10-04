// Skein design preview: agent invoke (#549). Injected after picker.js and
// editor.js, before proposals.js (which deletes the one-shot
// `window.__skeinPickerApi` this reads). Handles the host's `invoke` message:
// resolve the selector exactly like showElement, and only on a single match
// (and not while the user is picking) perform a tap or an edge swipe with
// synthetic events, then answer an `invoked` beacon.
(() => {
	const api = window.__skeinPickerApi;
	if (!api || typeof api.post !== "function") return;
	const { HOST, post, describe, isOverlay, isPicking } = api;

	const PID = 7; // synthetic pointer id; real mouse is 1, touch.js uses 2
	const STEPS = 8;
	const STEP_MS = 16;
	const SETTLE_MS = 300;
	const INSET = 4;

	const base = (type, x, y, pointerType, down, extra) => ({
		bubbles: true,
		cancelable: true,
		composed: true,
		view: window,
		clientX: x,
		clientY: y,
		button: 0,
		buttons: down ? 1 : 0,
		...extra,
		...(type.startsWith("pointer")
			? { pointerId: PID, isPrimary: true, pointerType, width: 1, height: 1 }
			: {}),
	});
	const fire = (target, type, x, y, pointerType, down) => {
		const Ctor = type.startsWith("pointer") ? PointerEvent : MouseEvent;
		target.dispatchEvent(new Ctor(type, base(type, x, y, pointerType, down)));
	};

	// Mutations Skein's own overlay nodes cause are not the page reacting.
	const own = (rec) => {
		// A Text node (characterData) has no closest(); test its parent element.
		const t = rec.target;
		const host = t.nodeType === 1 ? t : t.parentElement;
		if (host && isOverlay(host)) return true;
		const nodes = [...rec.addedNodes, ...rec.removedNodes];
		return (
			rec.type === "childList" &&
			nodes.length > 0 &&
			nodes.every((n) => n.nodeType === 1 && isOverlay(n)) // removed nodes are detached, but still marked
		);
	};

	const watch = () => {
		let changed = false;
		let mo = null;
		try {
			mo = new MutationObserver((recs) => {
				if (!changed && recs.some((r) => !own(r))) changed = true;
			});
			mo.observe(document.documentElement, {
				subtree: true,
				childList: true,
				attributes: true,
				characterData: true,
			});
		} catch (_) {
			mo = null;
		}
		return (done) => {
			setTimeout(() => {
				if (mo) {
					const rest = mo.takeRecords();
					if (!changed && rest.some((r) => !own(r))) changed = true;
					mo.disconnect();
				}
				done(changed);
			}, SETTLE_MS);
		};
	};

	const tap = (el, pt, finish) => {
		const r = el.getBoundingClientRect();
		const x = r.left + r.width / 2;
		const y = r.top + r.height / 2;
		fire(el, "pointerdown", x, y, pt, true);
		fire(el, "mousedown", x, y, pt, true);
		fire(el, "pointerup", x, y, pt, false);
		fire(el, "mouseup", x, y, pt, false);
		el.click();
		finish();
	};

	const swipe = (el, m, pt, finish) => {
		el.scrollIntoView({ block: "nearest", inline: "nearest" });
		const r = el.getBoundingClientRect();
		const dist = Number.isFinite(m.distance) ? m.distance : 120;
		const dir = m.direction;
		const cx = r.left + r.width / 2;
		const cy = r.top + r.height / 2;
		let sx = cx;
		let sy = cy;
		let dx = 0;
		let dy = 0;
		if (dir === "right") [sx, dx] = [r.left + INSET, dist];
		else if (dir === "left") [sx, dx] = [r.right - INSET, -dist];
		else if (dir === "down") [sy, dy] = [r.top + INSET, dist];
		else [sy, dy] = [r.bottom - INSET, -dist];
		const hit = document.elementFromPoint(sx, sy);
		const target = hit && el.contains(hit) ? hit : el;
		const at = (i) => [sx + (dx * i) / STEPS, sy + (dy * i) / STEPS];
		fire(target, "pointerdown", sx, sy, pt, true);
		let i = 0;
		const next = () => {
			i += 1;
			const [x, y] = at(i);
			if (i <= STEPS) {
				fire(target, "pointermove", x, y, pt, true);
				setTimeout(next, STEP_MS);
			} else {
				const [ex, ey] = at(STEPS);
				fire(target, "pointerup", ex, ey, pt, false);
				finish();
			}
		};
		setTimeout(next, STEP_MS);
	};

	const invoke = (m) => {
		const reply = (extra) => post({ type: "invoked", requestId: m.requestId, ...extra });
		if (isPicking?.()) {
			reply({ count: 0, element: null, busy: true });
			return;
		}
		let nodes = [];
		let invalid = false;
		try {
			nodes = Array.from(document.querySelectorAll(String(m.selector))).filter(
				(el) => !isOverlay(el),
			);
		} catch (_) {
			nodes = [];
			invalid = true;
		}
		const el = nodes.length === 1 ? nodes[0] : null;
		if (!el) {
			reply({ count: nodes.length, element: null, ...(invalid ? { invalid: true } : {}) });
			return;
		}
		const pt = m.pointerType === "touch" ? "touch" : "mouse";
		const settle = watch();
		const finish = () =>
			settle((domChanged) => {
				let element = null;
				try {
					element = describe(el);
				} catch (_) {
					// Detached by the action; the beacon still answers.
				}
				reply({ count: 1, element, domChanged });
			});
		if (m.action === "swipe") swipe(el, m, pt, finish);
		else tap(el, pt, finish);
	};

	window.addEventListener("message", (e) => {
		if (e.source !== window.parent) return;
		const m = e.data;
		if (!m || typeof m !== "object" || m.source !== HOST || m.v !== 1) return;
		if (m.type !== "invoke") return;
		try {
			invoke(m);
		} catch (_) {
			// A bad host message must never break the page.
		}
	});
})();
