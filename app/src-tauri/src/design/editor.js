// Skein design preview: edit mode (#436), injected right after picker.js.
// Picks one element, lets the host tweak allowlisted styles, drag/nudge it,
// resize it and retype its text; every change goes up as an `edit-change`
// beacon. Nothing is written to source: the host turns the changes into a
// proposal on a review thread. tokens.js (before this script) scans the page's
// design tokens; proposals.js (next script) previews pending
// proposals and takes what it needs from `api.editor`, set below.
// picker.js hands over the few internals it shares as `window.__skeinPickerApi`;
// proposals.js, the LAST script, deletes it. Envelope checks are repeated here.
(() => {
	const api = window.__skeinPickerApi;
	if (!api?.tokens) return;
	const { OVERLAY, HOST, post, describe, isOverlay, root, rectOf, flush } = api;
	const { scanTokens, sameValue } = api.tokens;

	// Keep identical to PROPERTY_ALLOWLIST in app/src/designProposal.ts and in
	// app/src-tauri/src/review_surface/proposal.rs (a vitest checks all three).
	const ALLOWLIST = [
		"width",
		"height",
		"margin-top",
		"margin-right",
		"margin-bottom",
		"margin-left",
		"padding-top",
		"padding-right",
		"padding-bottom",
		"padding-left",
		"gap",
		"row-gap",
		"column-gap",
		"color",
		"background-color",
		"border-color",
		"border-radius",
		"font-family",
		"font-size",
		"font-weight",
		"line-height",
		"letter-spacing",
		"opacity",
	];
	const MAX_VAL = 200;
	const MAX_TEXT = 2000;
	const NO_TEXT_TAGS = ["input", "textarea", "select", "img", "svg", "video", "canvas", "br", "hr"];

	const r2 = (n) => Math.round(n * 100) / 100;
	const hasCtrl = (s) => {
		for (let i = 0; i < s.length; i++) {
			const c = s.charCodeAt(i);
			if (c < 32 || c === 127) return true;
		}
		return false;
	};
	const cleanVal = (v) => (typeof v === "string" && v.length <= MAX_VAL && !hasCtrl(v) ? v : null);
	const px = (n) => `${r2(n)}px`;

	// ---- state ---------------------------------------------------------------

	let mode = null; // null | "pick" | "edit"
	let hover = null;
	let el = null;
	let orig = null; // { style, text, textOnly, transform }
	let computed = {};
	let tokens = [];
	let off = { dx: 0, dy: 0 };
	let handles = [];
	let textEdit = null; // { before, attr }
	let gesture = null;

	const textOnly = (e) =>
		e.childElementCount === 0 &&
		!NO_TEXT_TAGS.includes(e.tagName.toLowerCase()) &&
		(e.textContent || "").trim() !== "";

	const tokenFor = (prop, to) => {
		const hit = tokens.find((t) => t.value !== "" && sameValue(prop, t.value, to));
		return hit ? hit.name : undefined;
	};

	const emitStyle = (property, to) => {
		const change = { kind: "style", property, from: computed[property] ?? "", to };
		const token = tokenFor(property, to);
		if (token) change.token = token;
		post({ type: "edit-change", change });
	};
	const emitOffset = () => post({ type: "edit-change", change: { kind: "offset", ...off } });

	// ---- selection and handles --------------------------------------------

	const placeHandles = () => {
		if (!el) return;
		const r = rectOf(el);
		for (const h of handles) {
			h.node.style.left = `${r.x + (r.w * (h.dx + 1)) / 2}px`;
			h.node.style.top = `${r.y + (r.h * (h.dy + 1)) / 2}px`;
		}
	};
	const buildHandles = () => {
		for (const dy of [-1, 0, 1]) {
			for (const dx of [-1, 0, 1]) {
				if (dx === 0 && dy === 0) continue;
				const node = document.createElement("div");
				node.setAttribute(OVERLAY, "handle");
				const diag = dx !== 0 && dy !== 0;
				const cur = diag ? (dx === dy ? "nwse" : "nesw") : dx !== 0 ? "ew" : "ns";
				node.style.cssText = `position:absolute;width:10px;height:10px;box-sizing:border-box;background:#fff;border:2px solid #4f8cff;pointer-events:auto;cursor:${cur}-resize;transform:translate(-50%,-50%);`;
				node.addEventListener("mousedown", (e) => startResize(e, dx, dy));
				root().appendChild(node);
				handles.push({ node, dx, dy });
			}
		}
		placeHandles();
	};

	const select = (target) => {
		el = target;
		const cs = getComputedStyle(el);
		computed = {};
		for (const p of ALLOWLIST) computed[p] = cs.getPropertyValue(p).slice(0, MAX_VAL);
		orig = {
			style: el.getAttribute("style"),
			text: el.textContent || "",
			textOnly: textOnly(el),
			transform: el.style.transform || "",
		};
		tokens = scanTokens();
		off = { dx: 0, dy: 0 };
		mode = "edit";
		if (hover) hover.remove();
		hover = null;
		buildHandles();
		post({ type: "edit-picked", element: describe(el), computed, tokens });
	};

	// ---- gestures ------------------------------------------------------------

	const applyOffset = () => {
		const t = `translate(${off.dx}px, ${off.dy}px) ${orig.transform}`.trim();
		el.style.transform = t;
		flush();
		placeHandles();
	};

	const trackGesture = (move, end) => {
		const onMove = (e) => {
			e.preventDefault();
			move(e);
		};
		const onUp = (e) => {
			e.preventDefault();
			e.stopPropagation();
			document.removeEventListener("mousemove", onMove, true);
			document.removeEventListener("mouseup", onUp, true);
			gesture = null;
			end();
		};
		document.addEventListener("mousemove", onMove, true);
		document.addEventListener("mouseup", onUp, true);
		gesture = () => {
			document.removeEventListener("mousemove", onMove, true);
			document.removeEventListener("mouseup", onUp, true);
		};
	};

	const startDrag = (e) => {
		const sx = e.clientX;
		const sy = e.clientY;
		const base = { ...off };
		let moved = false;
		trackGesture(
			(m) => {
				moved = true;
				off = { dx: r2(base.dx + m.clientX - sx), dy: r2(base.dy + m.clientY - sy) };
				applyOffset();
			},
			() => {
				if (moved) emitOffset();
			},
		);
	};

	function startResize(e, dx, dy) {
		if (mode !== "edit" || !el) return;
		e.preventDefault();
		e.stopPropagation();
		const sx = e.clientX;
		const sy = e.clientY;
		const cs = getComputedStyle(el);
		const w0 = Number.parseFloat(cs.width) || 0;
		const h0 = Number.parseFloat(cs.height) || 0;
		let moved = false;
		trackGesture(
			(m) => {
				moved = true;
				if (dx !== 0) el.style.width = px(Math.max(0, w0 + dx * (m.clientX - sx)));
				if (dy !== 0) el.style.height = px(Math.max(0, h0 + dy * (m.clientY - sy)));
				flush();
				placeHandles();
			},
			() => {
				if (!moved) return;
				if (dx !== 0) emitStyle("width", el.style.width);
				if (dy !== 0) emitStyle("height", el.style.height);
			},
		);
	}

	const commitText = (keep) => {
		if (!textEdit) return;
		const { before, attr } = textEdit;
		textEdit = null;
		if (attr === null) el.removeAttribute("contenteditable");
		else el.setAttribute("contenteditable", attr);
		const after = el.textContent || "";
		if (!keep) el.textContent = before;
		else if (after !== before && after.length <= MAX_TEXT && orig.text.length <= MAX_TEXT) {
			post({ type: "edit-change", change: { kind: "text", from: orig.text, to: after } });
		}
		flush();
		placeHandles();
	};
	const startText = () => {
		if (textEdit || !orig.textOnly) return;
		textEdit = { before: el.textContent || "", attr: el.getAttribute("contenteditable") };
		el.setAttribute("contenteditable", "true");
		el.focus();
		el.addEventListener("blur", () => commitText(true), { once: true });
	};

	// ---- pick + edit listeners -------------------------------------------

	const swallow = (e) => {
		if (isOverlay(e.target)) return;
		if (textEdit && el?.contains(e.target)) return; // caret placement
		e.preventDefault();
		e.stopPropagation();
	};
	const onMove = (e) => {
		const t = e.target;
		if (mode !== "pick" || t?.nodeType !== 1 || isOverlay(t)) return;
		if (!hover) {
			hover = document.createElement("div");
			hover.setAttribute(OVERLAY, "hover");
			hover.style.cssText =
				"position:absolute;box-sizing:border-box;border:2px solid #8a5cf5;background:rgba(138,92,245,.12);pointer-events:none;";
			root().appendChild(hover);
		}
		const r = rectOf(t);
		hover.style.left = `${r.x}px`;
		hover.style.top = `${r.y}px`;
		hover.style.width = `${r.w}px`;
		hover.style.height = `${r.h}px`;
	};
	const onClick = (e) => {
		if (isOverlay(e.target)) return;
		if (textEdit && el?.contains(e.target)) return;
		e.preventDefault();
		e.stopPropagation();
		if (mode === "pick" && e.target?.nodeType === 1) select(e.target);
	};
	const onDown = (e) => {
		if (isOverlay(e.target)) return;
		if (mode === "edit" && !textEdit && el.contains(e.target) && e.button === 0) {
			e.preventDefault();
			e.stopPropagation();
			startDrag(e);
			return;
		}
		swallow(e);
	};
	const onDbl = (e) => {
		if (mode !== "edit" || !el.contains(e.target)) return;
		e.preventDefault();
		e.stopPropagation();
		startText();
	};
	const onKey = (e) => {
		if (textEdit) {
			if (e.key === "Escape") {
				e.preventDefault();
				e.stopPropagation();
				commitText(false);
			} else if (e.key === "Enter") {
				e.preventDefault();
				e.stopPropagation();
				commitText(true);
			}
			return;
		}
		if (e.key === "Escape") {
			e.preventDefault();
			e.stopPropagation();
			post({ type: "edit-cancelled" });
			return;
		}
		const step = e.shiftKey ? 8 : 1;
		const arrows = {
			ArrowLeft: [-step, 0],
			ArrowRight: [step, 0],
			ArrowUp: [0, -step],
			ArrowDown: [0, step],
		};
		const d = mode === "edit" ? arrows[e.key] : undefined;
		if (!d) return;
		e.preventDefault();
		e.stopPropagation();
		off = { dx: r2(off.dx + d[0]), dy: r2(off.dy + d[1]) };
		applyOffset();
		emitOffset();
	};
	const EVENTS = [
		["mousemove", onMove],
		["mousedown", onDown],
		["mouseup", swallow],
		["auxclick", swallow],
		["click", onClick],
		["dblclick", onDbl],
		["keydown", onKey],
	];

	const startEdit = () => {
		if (mode) return;
		mode = "pick";
		for (const [t, f] of EVENTS) document.addEventListener(t, f, true);
		// Previews must be gone before select() reads the element.
		for (const f of startHooks) f();
	};

	const endEdit = (revert) => {
		if (!mode) return;
		for (const [t, f] of EVENTS) document.removeEventListener(t, f, true);
		if (gesture) gesture();
		gesture = null;
		if (textEdit) commitText(!revert);
		if (hover) hover.remove();
		hover = null;
		for (const h of handles) h.node.remove();
		handles = [];
		if (el && revert) {
			if (orig.style === null) el.removeAttribute("style");
			else el.setAttribute("style", orig.style);
			if (orig.textOnly && el.textContent !== orig.text) el.textContent = orig.text;
		}
		el = null;
		orig = null;
		mode = null;
		flush();
		for (const f of endHooks) f();
	};

	const setStyle = (m) => {
		if (mode !== "edit" || !el || !ALLOWLIST.includes(m.property)) return;
		const value = cleanVal(m.value);
		if (value === null || value === "") return;
		el.style.setProperty(m.property, value);
		flush();
		placeHandles();
		emitStyle(m.property, value);
	};

	// What proposals.js shares: the allowlist and value checks, whether edit
	// mode is active (previews wait for it to end), and a hook for its end.
	const startHooks = [];
	const endHooks = [];
	api.editor = {
		ALLOWLIST,
		MAX_TEXT,
		cleanVal,
		textOnly,
		active: () => mode !== null,
		onStart: (f) => startHooks.push(f),
		onEnd: (f) => endHooks.push(f),
	};

	// ---- host messages -------------------------------------------------------

	window.addEventListener("message", (e) => {
		if (e.source !== window.parent) return;
		const m = e.data;
		if (!m || typeof m !== "object" || m.source !== HOST || m.v !== 1) return;
		try {
			switch (m.type) {
				case "edit-start":
					startEdit();
					break;
				case "edit-set":
					setStyle(m);
					break;
				case "edit-end":
					endEdit(m.revert !== false);
					break;
			}
		} catch (_) {
			// A bad host message must never break the page.
		}
	});
})();
