// Skein design pane: touch mode (#529). Injected after picker.js when the
// preview URL carries touch=1. Turns primary-button mouse drags into touch
// gestures (pointer + touch events, pointerType "touch"), without touching
// picker.js or editor.js: those swallow mouse events on `document` in
// capture phase, and this script listens one level further in, on the root
// element, so in pick and edit mode the mouse events never reach it.
(() => {
	if (window.__skeinTouch) return;
	window.__skeinTouch = true;

	const OVERLAY = "data-skein-overlay";
	const PID = 2; // synthetic pointer id; the real mouse is 1
	const TAP_SLOP = 10;
	const root = document.documentElement;
	// picker.js hands this over for one tick; proposals.js deletes it later.
	const api = window.__skeinPickerApi;

	let picking = false;
	let g = null; // the active gesture
	let captured = null;
	let swallowClick = null;
	let dot = null;
	let style = null;

	const isOverlay = (el) => !!el?.closest?.(`[${OVERLAY}]`);
	const editing = () => {
		try {
			return !!api?.editor?.active?.();
		} catch (_) {
			return false;
		}
	};
	const standDown = () => picking || editing();
	const guard =
		(fn) =>
		(...a) => {
			try {
				fn(...a);
			} catch (_) {
				// Touch mode must never break the page.
			}
		};
	const stop = (e) => {
		e.stopImmediatePropagation();
	};

	// ---- feature detection a prototype may do -----------------------------
	try {
		Object.defineProperty(navigator, "maxTouchPoints", { get: () => 5, configurable: true });
	} catch (_) {}
	try {
		if (!("ontouchstart" in window)) window.ontouchstart = null;
		if (!("ontouchstart" in document)) document.ontouchstart = null;
	} catch (_) {}
	try {
		const P = Element.prototype;
		const { setPointerCapture: set, releasePointerCapture: rel, hasPointerCapture: has } = P;
		P.setPointerCapture = function (id, ...rest) {
			if (id !== PID) return set.call(this, id, ...rest);
			captured = this;
		};
		P.releasePointerCapture = function (id, ...rest) {
			if (id !== PID) return rel.call(this, id, ...rest);
			if (captured === this) captured = null;
		};
		P.hasPointerCapture = function (id, ...rest) {
			if (id !== PID) return has.call(this, id, ...rest);
			return !!g && this === (captured || g.target);
		};
	} catch (_) {}

	// ---- indicator + cursor ------------------------------------------------
	// Toggled through the overlay elements only (never an attribute on <html>):
	// picker.js's MutationObserver ignores mutations inside overlay elements.
	const CURSOR_RULE = "html, html * { cursor: none !important }";
	const ensureUi = () => {
		if (!document.body) return;
		if (dot) {
			// A prototype replacing nodes may have dropped them.
			if (!dot.isConnected) root.append(style, dot);
			return;
		}
		style = document.createElement("style");
		style.setAttribute(OVERLAY, "touch-style");
		dot = document.createElement("div");
		dot.setAttribute(OVERLAY, "touch");
		dot.style.cssText =
			"position:fixed;left:0;top:0;width:20px;height:20px;margin:-10px 0 0 -10px;border-radius:50%;" +
			"background:rgba(128,128,128,.4);border:1px solid rgba(80,80,80,.6);pointer-events:none;" +
			"z-index:2147483647;display:none;box-sizing:border-box";
		root.append(style, dot); // not <body>: it would shift body > * and :last-child
		syncUi();
	};
	let lastOff = false;
	const syncUi = () => {
		lastOff = standDown();
		if (lastOff && g) endGesture(true); // pick/edit began mid-gesture
		if (!dot) return;
		const off = lastOff;
		style.textContent = off ? "" : CURSOR_RULE;
		if (off) dot.style.display = "none";
		dot.style.background = g ? "rgba(70,70,70,.65)" : "rgba(128,128,128,.4)";
	};
	const moveDot = (x, y) => {
		ensureUi();
		if (!dot || standDown()) return;
		dot.style.transform = `translate(${x}px,${y}px)`;
		dot.style.display = "block";
	};
	const hideDot = () => {
		if (dot) dot.style.display = "none";
	};

	// ---- event construction --------------------------------------------------
	const pointInit = (x, y, sx, sy, buttons) => ({
		bubbles: true,
		cancelable: true,
		composed: true,
		view: window,
		pointerId: PID,
		pointerType: "touch",
		isPrimary: true,
		width: 20,
		height: 20,
		pressure: buttons ? 0.5 : 0,
		clientX: x,
		clientY: y,
		screenX: sx,
		screenY: sy,
		button: 0,
		buttons,
	});
	const pointer = (type, t, x, y, sx, sy, buttons) => {
		const e = new PointerEvent(type, pointInit(x, y, sx, sy, buttons));
		t.dispatchEvent(e);
		return e;
	};
	const plainTouch = (t, x, y, sx, sy) => ({
		identifier: 1,
		target: t,
		clientX: x,
		clientY: y,
		pageX: x + window.scrollX,
		pageY: y + window.scrollY,
		screenX: sx,
		screenY: sy,
		radiusX: 10,
		radiusY: 10,
		rotationAngle: 0,
		force: 0.5,
	});
	const touch = (type, t, x, y, sx, sy, ended) => {
		const init = { bubbles: true, cancelable: true, composed: true };
		let e = null;
		try {
			if (typeof Touch === "function" && typeof TouchEvent === "function") {
				const pt = new Touch(plainTouch(t, x, y, sx, sy));
				e = new TouchEvent(type, {
					...init,
					touches: ended ? [] : [pt],
					targetTouches: ended ? [] : [pt],
					changedTouches: [pt],
				});
			}
		} catch (_) {
			e = null;
		}
		if (!e) {
			// WKWebView (and Chromium without a touchscreen) may lack the constructors.
			const pt = plainTouch(t, x, y, sx, sy);
			e = new Event(type, init);
			const lists = {
				touches: ended ? [] : [pt],
				targetTouches: ended ? [] : [pt],
				changedTouches: [pt],
				altKey: false,
				ctrlKey: false,
				metaKey: false,
				shiftKey: false,
			};
			for (const k of Object.keys(lists)) {
				Object.defineProperty(e, k, { value: lists[k], configurable: true });
			}
		}
		t.dispatchEvent(e);
		return e;
	};
	const mouse = (type, t, g0) => {
		t.dispatchEvent(
			new MouseEvent(type, {
				bubbles: true,
				cancelable: true,
				composed: true,
				view: window,
				button: 0,
				buttons: type === "mouseup" ? 0 : 1,
				clientX: g0.x,
				clientY: g0.y,
				screenX: g0.sx,
				screenY: g0.sy,
			}),
		);
	};

	// ---- scrolling -------------------------------------------------------------
	const touchActionNone = (el) => {
		for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
			if (getComputedStyle(n).touchAction === "none") return true;
		}
		return false;
	};
	const scrollable = (el, vertical, delta) => {
		for (let n = el; n && n.nodeType === 1; n = n.parentElement) {
			const cs = getComputedStyle(n);
			const ov = vertical ? cs.overflowY : cs.overflowX;
			if (!/auto|scroll|overlay/.test(ov)) continue;
			const size = vertical ? n.clientHeight : n.clientWidth;
			const full = vertical ? n.scrollHeight : n.scrollWidth;
			const pos = vertical ? n.scrollTop : n.scrollLeft;
			if (full > size && (delta < 0 ? pos > 0 : pos < full - size)) return n;
		}
		// The viewport's overflow comes from <html>, or from <body> if <html> is visible.
		const vp = (el) => (vertical ? getComputedStyle(el).overflowY : getComputedStyle(el).overflowX);
		let o = vp(root);
		if (o === "visible" && document.body) o = vp(document.body);
		return o === "hidden" || o === "clip" ? null : document.scrollingElement;
	};
	const dragScroll = (dx, dy) => {
		// A finger drags the content the other way.
		if (dx) {
			const s = scrollable(g.target, false, -dx);
			if (s) s.scrollLeft -= dx;
		}
		if (dy) {
			const s = scrollable(g.target, true, -dy);
			if (s) s.scrollTop -= dy;
		}
	};

	// ---- gesture -----------------------------------------------------------------
	const startGesture = (e) => {
		const t = e.target;
		g = {
			target: t,
			x: e.clientX,
			y: e.clientY,
			sx: e.screenX,
			sy: e.screenY,
			x0: e.clientX,
			y0: e.clientY,
			dist: 0,
			prevented: false,
			noScroll: touchActionNone(t),
		};
		const p = pointer("pointerdown", t, g.x, g.y, g.sx, g.sy, 1);
		const ts = touch("touchstart", t, g.x, g.y, g.sx, g.sy, false);
		// Only touch events gate click and scrolling; pointerdown only the compat mouse events.
		g.prevented = ts.defaultPrevented;
		g.ptrPrevented = p.defaultPrevented;
		syncUi();
	};
	const moveGesture = (e) => {
		const px = g.x;
		const py = g.y;
		g.x = e.clientX;
		g.y = e.clientY;
		g.sx = e.screenX;
		g.sy = e.screenY;
		g.dist = Math.max(g.dist, Math.hypot(g.x - g.x0, g.y - g.y0));
		// Implicit capture: moves go to the element the touch started on.
		pointer("pointermove", g.target, g.x, g.y, g.sx, g.sy, 1);
		const tm = touch("touchmove", g.target, g.x, g.y, g.sx, g.sy, false);
		if (!g.prevented && !g.noScroll && !tm.defaultPrevented) dragScroll(g.x - px, g.y - py);
	};
	const armClickSwallow = () => {
		const h = (ev) => {
			stop(ev);
			ev.preventDefault();
			disarm();
		};
		const disarm = () => {
			root.removeEventListener("click", h, true);
			clearTimeout(timer);
			swallowClick = null;
		};
		const timer = setTimeout(disarm, 400);
		root.addEventListener("click", h, true);
		swallowClick = disarm;
	};
	const endGesture = (cancel) => {
		const s = g;
		g = null;
		captured = null;
		if (cancel) {
			pointer("pointercancel", s.target, s.x, s.y, s.sx, s.sy, 0);
			touch("touchcancel", s.target, s.x, s.y, s.sx, s.sy, true);
		} else {
			pointer("pointerup", s.target, s.x, s.y, s.sx, s.sy, 0);
			const te = touch("touchend", s.target, s.x, s.y, s.sx, s.sy, true);
			const tap = s.dist < TAP_SLOP;
			if (tap && !s.prevented && !s.ptrPrevented && !te.defaultPrevented) {
				// As a real touch does: compat mouse events, then the native click.
				mouse("mousedown", s.target, s);
				mouse("mouseup", s.target, s);
			} else if (!tap || s.prevented || te.defaultPrevented) {
				armClickSwallow();
			}
		}
		syncUi();
	};

	// Accepted limits: a native `click` keeps pointerType "mouse", and a prototype's
	// own window/document capture listeners still see the real mouse events.
	// ---- listeners (capture, on <html>) ----------------------------------------------
	const HOVER = ["mouseover", "mouseout", "mouseenter", "mouseleave"];
	const on = (type, fn) => root.addEventListener(type, guard(fn), true);

	// The real mouse pointer sequence is hidden from the prototype. Pointer
	// events precede mousedown and picker.js does not swallow them, so they
	// are passed through while standing down and eaten otherwise.
	for (const type of ["pointerdown", "pointerup", "pointercancel"]) {
		on(type, (e) => {
			if (!e.isTrusted || e.pointerType !== "mouse" || standDown() || isOverlay(e.target)) return;
			if (e.button === 0 || g) stop(e);
		});
	}
	for (const type of ["pointermove", "pointerover", "pointerout", "pointerenter", "pointerleave"]) {
		on(type, (e) => {
			if (!e.isTrusted || e.pointerType !== "mouse" || standDown() || isOverlay(e.target)) return;
			stop(e);
		});
	}
	for (const type of HOVER) {
		on(type, (e) => {
			if (!e.isTrusted || standDown() || isOverlay(e.target)) return;
			stop(e);
		});
	}
	on("mousedown", (e) => {
		if (!e.isTrusted || standDown() || isOverlay(e.target)) return;
		if (e.button !== 0) return;
		if (swallowClick) swallowClick();
		stop(e); // not preventDefault: inputs must still focus
		startGesture(e);
	});
	on("mousemove", (e) => {
		if (!e.isTrusted || standDown()) return;
		moveDot(e.clientX, e.clientY);
		if (g && e.buttons === 0) {
			// The button was released outside the frame.
			stop(e);
			endGesture(true);
			return;
		}
		stop(e);
		if (g) moveGesture(e);
	});
	on("mouseup", (e) => {
		if (!e.isTrusted || !g || e.button !== 0) return;
		stop(e);
		g.x = e.clientX;
		g.y = e.clientY;
		g.sx = e.screenX;
		g.sy = e.screenY;
		endGesture(false);
	});
	for (const type of ["selectstart", "dragstart"]) {
		on(type, (e) => {
			if (g) e.preventDefault();
		});
	}
	root.addEventListener("mouseleave", guard(hideDot));
	window.addEventListener(
		"blur",
		guard(() => {
			if (g) endGesture(true);
		}),
	);

	// ---- mirror pick mode, for the indicator only ----------------------------------
	window.addEventListener(
		"message",
		guard((e) => {
			const m = e.data;
			if (e.source !== window.parent || !m || m.source !== "skein-host" || m.v !== 1) return;
			if (m.type === "pick-start") picking = true;
			else if (m.type === "pick-cancel") picking = false;
			else return;
			syncUi();
		}),
	);
	// picker.js leaves pick mode on a non-overlay click and on Escape. Observed
	// from window capture, which runs before picker.js stops either event.
	window.addEventListener(
		"click",
		guard((e) => {
			if (picking && !isOverlay(e.target)) {
				picking = false;
				syncUi();
			}
		}),
		true,
	);
	// Edit mode has no message to mirror; notice its flip on any mouse move.
	window.addEventListener(
		"mousemove",
		guard(() => {
			if (standDown() !== lastOff) syncUi();
		}),
		true,
	);
	window.addEventListener(
		"keydown",
		guard((e) => {
			if (picking && e.key === "Escape") {
				picking = false;
				syncUi();
			}
		}),
		true,
	);

	if (document.body) ensureUi();
	else document.addEventListener("DOMContentLoaded", guard(ensureUi));
})();
