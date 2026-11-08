// Skein design preview: the script injected into every served HTML page.
// Beacons go up (source "skein-design"); the host talks down with
// source "skein-host" (#434): pick mode, locate, pins, highlight.
(() => {
	const SOURCE = "skein-design";
	const HOST = "skein-host";
	const MAX = 500;
	const OVERLAY = "data-skein-overlay";
	const clip = (s) => String(s == null ? "" : s).slice(0, MAX);
	const post = (msg) => {
		try {
			window.parent.postMessage({ source: SOURCE, v: 1, ...msg }, "*");
		} catch (_) {
			// Nothing useful to do if the parent is gone.
		}
	};

	document.addEventListener("DOMContentLoaded", () => {
		post({ type: "ready", href: clip(location.href) });
	});

	// Capture phase: resource load errors do not bubble.
	window.addEventListener(
		"error",
		(e) => {
			const t = e.target;
			if (t && t !== window && t.nodeType === 1) {
				post({
					type: "resource-error",
					tag: clip(t.tagName.toLowerCase()),
					url: clip(t.src || t.href),
				});
				return;
			}
			post({
				type: "script-error",
				message: clip(e.message),
				url: clip(e.filename),
				line: e.lineno,
			});
		},
		true,
	);

	window.addEventListener("unhandledrejection", (e) => {
		const r = e.reason;
		post({ type: "script-error", message: clip(r?.message ?? r) });
	});

	// ---- per-file JSX source -------------------------------------------
	// Babel's built-in transform-react-jsx-source emits a shared
	// `var _jsxFileName` per script; @babel/standalone runs each
	// <script type="text/babel" src> as a classic script, so they all
	// share one global and the last file loaded wins. Override the plugin
	// under the SAME name (rewrite.rs's data-plugins keeps working, and a
	// failed registration falls back to the built-in) with one that
	// inlines the filename as a literal.
	let sourceRegistered = false;
	const registerSource = () => {
		try {
			if (sourceRegistered) return;
			const B = window.Babel;
			if (!B || typeof B.registerPlugin !== "function") return;
			B.registerPlugin("transform-react-jsx-source", ({ types: t }) => ({
				visitor: {
					JSXOpeningElement(path, state) {
						const node = path.node;
						if (!node.loc) return;
						const has = node.attributes.some(
							(a) => t.isJSXAttribute(a) && a.name && a.name.name === "__source",
						);
						if (has) return;
						const fileName = state.filename || state.file?.opts?.filename || "";
						node.attributes.push(
							t.jsxAttribute(
								t.jsxIdentifier("__source"),
								t.jsxExpressionContainer(
									t.objectExpression([
										t.objectProperty(t.identifier("fileName"), t.stringLiteral(fileName)),
										t.objectProperty(
											t.identifier("lineNumber"),
											t.numericLiteral(node.loc.start.line),
										),
										t.objectProperty(
											t.identifier("columnNumber"),
											t.numericLiteral(node.loc.start.column + 1),
										),
										t.objectProperty(
											t.identifier("endLineNumber"),
											t.numericLiteral(node.loc.end.line),
										),
									]),
								),
							),
						);
					},
				},
			}));
			sourceRegistered = true;
		} catch (_) {
			// Falls back to the built-in plugin.
		}
	};
	// Babel may already be loaded; if it loads later in <body>, a listener
	// on `document` fires before Babel's own `window` DOMContentLoaded
	// listener (which does the transforming) in the bubble path.
	registerSource();
	document.addEventListener("DOMContentLoaded", registerSource);

	// ---- element descriptors -------------------------------------------

	const isOverlay = (el) => !!el?.closest?.(`[${OVERLAY}]`);
	const ws = (s) =>
		String(s || "")
			.replace(/\s+/g, " ")
			.trim();
	const clipTo = (s, n) => String(s).slice(0, n);

	// Flutter's flt-semantic-node-<n> ids are layout counters, not identity.
	const unstableId = (id) =>
		window.__skeinHit?.isUnstableId ? window.__skeinHit.isUnstableId(id) : false;
	const stableId = (el) => {
		const id = el.getAttribute("id");
		return id && !unstableId(id) ? id : undefined;
	};

	const odIdOf = (el) =>
		el.getAttribute("data-od-id") || el.getAttribute("data-screen-label") || stableId(el);

	const textOf = (el) => {
		// Clone-free: overlay nodes live on documentElement, never inside
		// page elements, but guard anyway by skipping when inside one.
		if (isOverlay(el)) return "";
		const raw = typeof el.innerText === "string" ? el.innerText : el.textContent;
		return clipTo(ws(raw), 500);
	};

	const esc = (s) =>
		window.CSS && CSS.escape ? CSS.escape(s) : String(s).replace(/[^\w-]/g, "\\$&");

	const selectorOf = (el) => {
		const parts = [];
		let cur = el;
		while (cur && cur.nodeType === 1) {
			const tag = cur.tagName.toLowerCase();
			const id = stableId(cur);
			if (id && document.getElementById(id) === cur) {
				parts.unshift(`#${esc(id)}`);
				break;
			}
			if (cur === document.documentElement || tag === "body") {
				parts.unshift(tag);
				if (tag === "body") parts.unshift("html");
				break;
			}
			let n = 1;
			for (let s = cur.previousElementSibling; s; s = s.previousElementSibling) {
				if (s.tagName === cur.tagName) n++;
			}
			parts.unshift(`${tag}:nth-of-type(${n})`);
			cur = cur.parentElement;
		}
		return clipTo(parts.join(" > "), 1000);
	};

	const attrsOf = (el) => {
		const out = {};
		let count = 0;
		const keep = (name) =>
			[
				"class",
				"id",
				"role",
				"name",
				"type",
				"href",
				"src",
				"alt",
				"title",
				"data-od-id",
				"data-screen-label",
			].includes(name) || name.startsWith("aria-");
		for (const a of Array.from(el.attributes)) {
			if (count >= 16) break;
			if (!keep(a.name)) continue;
			out[clipTo(a.name, 64)] = clipTo(a.value, 300);
			count++;
		}
		return out;
	};

	const rectOf = (el) => {
		const r = el.getBoundingClientRect();
		return {
			x: Math.round(r.left + window.scrollX),
			y: Math.round(r.top + window.scrollY),
			w: Math.round(r.width),
			h: Math.round(r.height),
		};
	};

	// The element's OWN fiber only: walking up would name the parent
	// component's line, not this element's.
	const rawSourceOf = (el) => {
		try {
			const key = Object.keys(el).find((k) => k.startsWith("__reactFiber$"));
			const src = key && el[key]?._debugSource;
			if (!src || typeof src.fileName !== "string") return undefined;
			const out = { fileName: clipTo(src.fileName, 1000), lineNumber: src.lineNumber };
			if (src.columnNumber != null) out.columnNumber = src.columnNumber;
			if (src.endLineNumber != null) out.endLineNumber = src.endLineNumber;
			return out;
		} catch (_) {
			return undefined;
		}
	};

	// Current scroll, sent with picked/located so a parent picker can convert
	// this document's coordinates (#708).
	const scrollNow = () => ({ x: Math.round(window.scrollX), y: Math.round(window.scrollY) });

	let describe = (el) => {
		const d = {
			selector: selectorOf(el),
			tag: clipTo(el.tagName.toLowerCase(), 64),
			text: textOf(el),
			attrs: attrsOf(el),
			rect: rectOf(el),
		};
		const od = odIdOf(el);
		if (od) d.odId = clipTo(od, 200);
		const rs = rawSourceOf(el);
		if (rs) d.rawSource = rs;
		return d;
	};

	// ---- pick mode -------------------------------------------------------

	let picking = false;
	let box = null;
	let tag = null;

	const ensureOverlay = () => {
		let root = document.documentElement.querySelector(`:scope > [${OVERLAY}="root"]`);
		if (!root) {
			root = document.createElement("div");
			root.setAttribute(OVERLAY, "root");
			root.style.cssText =
				"position:absolute;left:0;top:0;width:0;height:0;margin:0;padding:0;border:0;z-index:2147483647;pointer-events:none;";
			document.documentElement.appendChild(root);
		}
		return root;
	};

	const moveBox = (el) => {
		if (!box) {
			box = document.createElement("div");
			box.setAttribute(OVERLAY, "hover");
			box.style.cssText =
				"position:absolute;box-sizing:border-box;border:2px solid #4f8cff;background:rgba(79,140,255,.12);pointer-events:none;";
			ensureOverlay().appendChild(box);
		}
		const r = rectOf(el);
		box.style.left = `${r.x}px`;
		box.style.top = `${r.y}px`;
		box.style.width = `${r.w}px`;
		box.style.height = `${r.h}px`;
	};

	// First ~120 chars of an element's text, walking text nodes with an early
	// exit so a huge subtree is never read whole.
	const shortText = (el) => {
		let out = "";
		const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
		for (let n = w.nextNode(); n && out.length < 120; n = w.nextNode()) out += n.nodeValue;
		return out.slice(0, 120);
	};

	const moveLabel = (el) => {
		const hit = window.__skeinHit;
		if (!hit || !box) return;
		let text = "";
		try {
			text = hit.labelOf({
				tag: el.tagName.toLowerCase(),
				role: el.getAttribute("role") || "",
				ariaLabel: el.getAttribute("aria-label") || "",
				text: shortText(el),
			});
		} catch (_) {
			return;
		}
		if (!tag) {
			tag = document.createElement("div");
			tag.setAttribute(OVERLAY, "label");
			tag.style.cssText =
				"position:absolute;box-sizing:border-box;background:#4f8cff;color:#fff;font:11px/1.4 system-ui,sans-serif;padding:1px 6px;white-space:nowrap;pointer-events:none;max-width:90vw;overflow:hidden;";
			ensureOverlay().appendChild(tag);
		}
		tag.textContent = text;
		const r = rectOf(el);
		const above = r.y - 18;
		const maxLeft = window.scrollX + document.documentElement.clientWidth - tag.offsetWidth;
		tag.style.left = `${Math.max(window.scrollX, Math.min(r.x, maxLeft))}px`;
		tag.style.top = `${above >= window.scrollY ? above : r.y + 2}px`;
	};

	// The element the pointer is over. On a Flutter web page the event
	// target is the canvas host, so resolve to the innermost flt-semantics
	// node whose box holds the pointer; anything else falls back to target.
	const targetAt = (e) => {
		const t = e.target;
		try {
			const hit = window.__skeinHit;
			if (!hit || !document.querySelector("flt-semantics")) return t;
			const inView =
				t === document.documentElement ||
				t === document.body ||
				(t?.nodeType === 1 && t.closest("flutter-view, flt-glass-pane, flt-semantics-host"));
			if (!inView) return t;
			const hosts = [
				"flutter-view",
				"flt-glass-pane",
				"flt-semantics-host",
				"flt-scene-host",
				"flt-scene",
				"canvas",
			];
			const isHost = (el) =>
				el === document.documentElement ||
				el === document.body ||
				hosts.includes(el?.localName) ||
				(el?.localName === "flt-semantics" && !!el.querySelector("flt-semantics"));
			const nodes = Array.from(document.querySelectorAll("flt-semantics")).slice(0, 2000);
			const seen = new Set(nodes);
			// Map the target / stack hits (e.g. a text span) to their node.
			const extra = [t, ...(document.elementsFromPoint?.(e.clientX, e.clientY) || [])];
			for (const x of extra) {
				const n = x?.nodeType === 1 ? x.closest("flt-semantics") : null;
				if (!n) continue;
				if (!seen.has(n)) {
					seen.add(n);
					nodes.push(n);
				}
			}
			const cands = nodes.map((n, order) => {
				let depth = 0;
				for (let p = n.parentElement; p; p = p.parentElement) {
					if (p.localName === "flt-semantics") depth++;
				}
				const b = n.getBoundingClientRect();
				return {
					rect: { left: b.left, top: b.top, width: b.width, height: b.height },
					depth,
					order,
				};
			});
			const i = hit.pickDeepest({ x: e.clientX, y: e.clientY }, cands);
			const winner = i >= 0 ? nodes[i] : null;
			return hit.refine(t, winner, { isHost, contains: (a, b) => a.contains(b) }) ?? t;
		} catch (_) {
			return t;
		}
	};

	let moveRaf = 0;
	let lastMove = null;
	const onMove = (e) => {
		lastMove = e;
		if (moveRaf) return;
		moveRaf = requestAnimationFrame(() => {
			moveRaf = 0;
			const ev = lastMove;
			lastMove = null;
			if (!picking || !ev) return;
			const el = targetAt(ev);
			if (el?.nodeType !== 1 || isOverlay(el)) return;
			moveBox(el);
			moveLabel(el);
		});
	};
	const onClick = (e) => {
		e.preventDefault();
		e.stopPropagation();
		const el = targetAt(e);
		if (el?.nodeType !== 1 || isOverlay(el)) return;
		endPick();
		post({ type: "picked", element: describe(el), scroll: scrollNow() });
	};
	const onKey = (e) => {
		if (e.key !== "Escape") return;
		e.preventDefault();
		e.stopPropagation();
		endPick();
		post({ type: "pick-cancelled" });
	};
	// The pointer left this document: drop the hover box so a parent and a
	// child picker never both show one.
	const onOut = (e) => {
		if (e.relatedTarget) return;
		lastMove = null;
		if (box) box.remove();
		box = null;
		if (tag) tag.remove();
		tag = null;
	};
	// Swallow the rest of the gesture so the page does not react to it.
	const swallow = (e) => {
		e.preventDefault();
		e.stopPropagation();
	};

	const startPick = () => {
		if (picking) return;
		picking = true;
		document.addEventListener("mousemove", onMove, true);
		document.addEventListener("mouseout", onOut, true);
		document.addEventListener("click", onClick, true);
		document.addEventListener("keydown", onKey, true);
		document.addEventListener("mousedown", swallow, true);
		document.addEventListener("mouseup", swallow, true);
		document.addEventListener("pointerdown", swallow, true);
		document.addEventListener("pointerup", swallow, true);
		document.addEventListener("auxclick", swallow, true);
	};
	function stopPick() {
		if (!picking) return;
		picking = false;
		document.removeEventListener("mousemove", onMove, true);
		document.removeEventListener("mouseout", onOut, true);
		document.removeEventListener("click", onClick, true);
		document.removeEventListener("keydown", onKey, true);
		document.removeEventListener("mousedown", swallow, true);
		document.removeEventListener("mouseup", swallow, true);
		document.removeEventListener("pointerdown", swallow, true);
		document.removeEventListener("pointerup", swallow, true);
		document.removeEventListener("auxclick", swallow, true);
		if (box) box.remove();
		box = null;
		if (tag) tag.remove();
		tag = null;
		lastMove = null;
	}

	// ---- locate ----------------------------------------------------------

	const safeQuery = (sel) => {
		try {
			const el = document.querySelector(sel);
			return el && !isOverlay(el) ? el : null;
		} catch (_) {
			return null;
		}
	};

	const locateOne = (a, all) => {
		const res = { bySelector: null, byOdId: [], byText: [], sameTag: [] };
		if (typeof a.selector === "string") {
			const el = safeQuery(a.selector);
			if (el) res.bySelector = describe(el);
		}
		const tag = typeof a.tag === "string" ? a.tag.toLowerCase() : "";
		if (typeof a.odId === "string" && a.odId) {
			for (const el of all) {
				if (res.byOdId.length >= 10) break;
				if (odIdOf(el) === a.odId) res.byOdId.push(describe(el));
			}
		}
		if (tag) {
			let same = [];
			try {
				same = Array.from(document.getElementsByTagName(tag));
			} catch (_) {
				same = [];
			}
			same = same.filter((el) => !isOverlay(el));
			const want = typeof a.text === "string" ? clipTo(ws(a.text), 500) : "";
			if (want) {
				for (const el of same) {
					if (res.byText.length >= 10) break;
					if (textOf(el) === want) res.byText.push(describe(el));
				}
			}
			for (const el of same.slice(0, 200)) res.sameTag.push(describe(el));
		}
		return res;
	};

	const locate = (msg) => {
		// describe() reads layout; one pass sees each element many times.
		const cache = new Map();
		const base = describe;
		describe = (el) => {
			let d = cache.get(el);
			if (!d) {
				d = base(el);
				cache.set(el, d);
			}
			return d;
		};
		try {
			locateAll(msg);
		} finally {
			describe = base;
		}
	};

	const locateAll = (msg) => {
		const anchors = Array.isArray(msg.anchors) ? msg.anchors.slice(0, 100) : [];
		const all = Array.from(document.querySelectorAll("*")).filter((el) => !isOverlay(el));
		const results = [];
		const { local, byFrame } = window.__skeinFrames.partitionAnchors(anchors);
		for (const a of local) {
			try {
				results.push({ id: a.id, found: locateOne(a, all) });
			} catch (_) {
				results.push({
					id: a.id,
					found: { bySelector: null, byOdId: [], byText: [], sameTag: [] },
				});
			}
		}
		const files = [location.href];
		try {
			for (const e of performance.getEntriesByType("resource")) {
				if (files.length >= 200) break;
				files.push(e.name);
			}
		} catch (_) {
			// Resource timing unavailable.
		}
		if (byFrame.length) relayFrames(byFrame, results, files, msg.requestId);
		else post({ type: "located", requestId: msg.requestId, results, files, scroll: scrollNow() });
	};

	// ---- child frames (#708) -----------------------------------------------
	// Child pickers are reached only by postMessage (the preview is sandboxed
	// to an opaque origin). A window counts as a child only once it has sent
	// a skein-design message AND is the contentWindow of one of our frames.

	const kids = new Set();
	const frameElOf = (w) => {
		for (const f of document.querySelectorAll("iframe,frame")) {
			if (f.contentWindow === w && !isOverlay(f)) return f;
		}
		return null;
	};
	const sendKid = (w, msg) => {
		try {
			w.postMessage({ source: HOST, v: 1, ...msg }, "*");
		} catch (_) {
			// Frame is gone.
		}
	};
	const eachKid = (fn) => {
		for (const w of kids) {
			if (frameElOf(w)) fn(w);
			else kids.delete(w);
		}
	};
	// The iframe's content-box origin in this document's coordinates.
	const boxOf = (f) => {
		const r = f.getBoundingClientRect();
		const cs = getComputedStyle(f);
		return {
			x: r.left + window.scrollX + f.clientLeft + (Number.parseFloat(cs.paddingLeft) || 0),
			y: r.top + window.scrollY + f.clientTop + (Number.parseFloat(cs.paddingTop) || 0),
		};
	};

	// End a pick here and in every child picker.
	const endPick = () => {
		stopPick();
		eachKid((w) => sendKid(w, { type: "pick-cancel" }));
	};

	const pendingSubs = new Map();
	let subSeq = 0;
	const relayFrames = (groups, results, files, requestId) => {
		const F = window.__skeinFrames;
		const mySubs = new Set();
		let done = false;
		const settle = (g, reply, frameEl) => {
			const byId = new Map();
			for (const r of Array.isArray(reply?.results) ? reply.results : []) {
				if (r) byId.set(r.id, r.found);
			}
			for (const a of g.anchors) {
				results.push({
					id: a.id,
					found:
						frameEl && byId.has(a.id)
							? F.relayLocated(byId.get(a.id), g.sel, boxOf(frameEl), reply.scroll)
							: F.emptyFound(),
				});
			}
			if (frameEl && Array.isArray(reply?.files)) {
				for (const f of reply.files) {
					if (files.length >= 200) break;
					if (typeof f === "string") files.push(f);
				}
			}
		};
		const finish = () => {
			if (done) return;
			done = true;
			clearTimeout(timer);
			for (const id of mySubs) {
				const p = pendingSubs.get(id);
				pendingSubs.delete(id);
				if (p) settle(p.g, null, null);
			}
			post({ type: "located", requestId, results, files, scroll: scrollNow() });
		};
		const timer = setTimeout(finish, 1500);
		for (const g of groups) {
			const el = safeQuery(g.sel);
			const win = el && /^i?frame$/i.test(el.tagName) ? el.contentWindow : null;
			if (!win || !kids.has(win)) {
				settle(g, null, null);
				continue;
			}
			const id = `skein-frame-${++subSeq}`;
			mySubs.add(id);
			pendingSubs.set(id, {
				g,
				win,
				cb: (reply) => {
					mySubs.delete(id);
					settle(g, reply, el);
					if (mySubs.size === 0) finish();
				},
			});
			sendKid(win, { type: "locate", requestId: id, anchors: g.anchors });
		}
		if (mySubs.size === 0) finish();
	};

	// Trust model: any direct child frame that speaks the protocol is trusted;
	// the worst it can do is end or fake the user's own in-progress pick.
	// Messages from child pickers. Only picked / pick-cancelled / located are
	// relayed (rewritten); dom-changed and scroll just re-measure our pins.
	window.addEventListener("message", (e) => {
		const m = e.data;
		if (e.source === window || e.source === window.parent) return;
		if (!m || typeof m !== "object" || m.source !== SOURCE || m.v !== 1) return;
		const f = frameElOf(e.source);
		if (!f) return;
		kids.add(e.source);
		switch (m.type) {
			case "ready":
				if (picking) sendKid(e.source, { type: "pick-start" });
				break;
			case "picked":
			case "pick-cancelled": {
				if (!picking) break;
				endPick();
				if (m.type === "pick-cancelled") post({ type: "pick-cancelled" });
				else if (m.element && typeof m.element === "object") {
					const d = window.__skeinFrames.prefixDescriptor(
						m.element,
						selectorOf(f),
						boxOf(f),
						m.scroll,
					);
					post({ type: "picked", element: d, scroll: scrollNow() });
				}
				break;
			}
			case "located": {
				const p = pendingSubs.get(m.requestId);
				if (p && p.win === e.source) {
					pendingSubs.delete(m.requestId);
					p.cb(m);
				}
				break;
			}
			case "dom-changed":
			case "scroll":
				domChanged();
				break;
		}
	});

	// ---- pins ------------------------------------------------------------

	const STATE_STYLE = {
		anchored: "background:#2f9e5b;border:2px solid #fff;",
		reanchored: "background:#d99a1e;border:2px solid #fff;",
		stale: "background:#7a7f87;border:2px solid #fff;opacity:.8;",
		lost: "background:transparent;color:#c0392b;border:2px dashed #c0392b;",
	};
	const pinNodes = new Map();

	// The latest pins, kept so they can be redrawn if the page ever drops
	// the overlay (it lives on <html>, but the page is arbitrary code).
	let lastPins = null;
	const setPins = (msg) => {
		lastPins = msg;
		const root = ensureOverlay();
		for (const n of pinNodes.values()) n.remove();
		pinNodes.clear();
		const pins = Array.isArray(msg.pins) ? msg.pins : [];
		for (const p of pins) {
			if (!p?.rect || !Number.isFinite(p.n)) continue;
			const { x, y } = p.rect;
			if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
			const node = document.createElement("div");
			node.setAttribute(OVERLAY, "pin");
			node.textContent = String(p.n);
			// biome-ignore lint/suspicious/noPrototypeBuiltins: Object.hasOwn needs Safari 15.4; this script is served untranspiled into the WebView
			node.style.cssText = `position:absolute;left:${x}px;top:${y}px;min-width:18px;height:18px;padding:0 4px;box-sizing:border-box;border-radius:9px;color:#fff;font:600 11px/14px system-ui,sans-serif;text-align:center;pointer-events:none;transform:translate(-50%,-50%);${Object.prototype.hasOwnProperty.call(STATE_STYLE, p.state) ? STATE_STYLE[p.state] : STATE_STYLE.anchored}`;
			root.appendChild(node);
			pinNodes.set(p.n, node);
		}
	};

	const highlight = (msg) => {
		const node = pinNodes.get(msg.n);
		if (!node) return;
		node.scrollIntoView({ block: "center", inline: "center", behavior: "smooth" });
		node.animate(
			[
				{ transform: "translate(-50%,-50%) scale(1)" },
				{ transform: "translate(-50%,-50%) scale(1.8)" },
				{ transform: "translate(-50%,-50%) scale(1)" },
			],
			{ duration: 600, iterations: 2 },
		);
	};

	// ---- dom-changed -------------------------------------------------------
	// Babel transpiles and React renders after DOMContentLoaded, so the page
	// the host located against at `ready` may be empty. Tell the host when
	// the DOM settles; our own overlay (pins, hover box, flash) is ignored
	// so drawing pins can never cause another round.

	const ownMutation = (m) => {
		if (isOverlay(m.target)) return true;
		if (m.type !== "childList") return false;
		const nodes = [...m.addedNodes, ...m.removedNodes];
		return nodes.length > 0 && nodes.every((n) => n.nodeType === 1 && n.hasAttribute(OVERLAY));
	};
	let domTimer = null;
	let mo = null;
	const settleHooks = [];
	const domChanged = () => {
		if (domTimer !== null) clearTimeout(domTimer);
		domTimer = setTimeout(() => {
			domTimer = null;
			for (const h of settleHooks) {
				try {
					h();
				} catch (_) {
					// A hook must never stop the beacon.
				}
			}
			if (lastPins && !document.documentElement.querySelector(`:scope > [${OVERLAY}="root"]`)) {
				setPins(lastPins);
			}
			post({ type: "dom-changed" });
		}, 250);
	};
	// Layout can settle with no DOM mutation: images and fonts arriving,
	// CSS transitions/animations ending, the page resizing itself. Pins use
	// measured rects, so those are dom-changed too.
	const layoutSignal = (e) => {
		if (e?.target && e.target.nodeType === 1 && isOverlay(e.target)) return;
		domChanged();
	};
	const startObserving = () => {
		for (const t of ["load", "transitionend", "animationend"]) {
			document.addEventListener(t, layoutSignal, true);
		}
		window.addEventListener("load", layoutSignal);
		try {
			document.fonts?.ready?.then(domChanged);
		} catch (_) {
			// No font loading API.
		}
		try {
			const ro = new ResizeObserver(domChanged);
			ro.observe(document.documentElement);
			if (document.body) ro.observe(document.body);
		} catch (_) {
			// No ResizeObserver.
		}
		mo = new MutationObserver((list) => {
			if (list.some((m) => !ownMutation(m))) domChanged();
		});
		mo.observe(document, {
			childList: true,
			subtree: true,
			characterData: true,
			attributes: true,
		});
		window.addEventListener("resize", domChanged);
	};
	if (document.readyState === "loading") {
		document.addEventListener("DOMContentLoaded", startObserving);
	} else {
		startObserving();
	}

	// ---- show element (agent, #512) -----------------------------------------
	// An outline that persists until the next showElement, Escape or a click
	// in the frame: the pane may be hidden when the agent asks, and the user
	// must still see it when they look.

	let shownEl = null;
	let shownBox = null;
	const clearShown = () => {
		shownEl = null;
		if (shownBox) shownBox.remove();
		shownBox = null;
	};
	// A dismissal by the user (click / Escape): tell the host so a hidden-pane
	// re-show does not bring the outline back.
	const dismissShown = () => {
		if (!shownEl) return;
		clearShown();
		post({ type: "shownCleared" });
	};
	const placeShown = () => {
		if (!shownEl) return;
		if (!shownEl.isConnected) {
			clearShown();
			return;
		}
		const root = ensureOverlay();
		if (!shownBox || shownBox.parentNode !== root) {
			if (shownBox) shownBox.remove();
			shownBox = document.createElement("div");
			shownBox.setAttribute(OVERLAY, "shown");
			shownBox.style.cssText =
				"position:absolute;box-sizing:border-box;border:3px solid #e5329b;background:rgba(229,50,155,.12);box-shadow:0 0 0 2px rgba(255,255,255,.8);pointer-events:none;";
			root.appendChild(shownBox);
		}
		const r = rectOf(shownEl);
		shownBox.style.left = `${r.x}px`;
		shownBox.style.top = `${r.y}px`;
		shownBox.style.width = `${r.w}px`;
		shownBox.style.height = `${r.h}px`;
	};
	settleHooks.push(placeShown);
	window.addEventListener("resize", placeShown);
	document.addEventListener("click", dismissShown, true);
	document.addEventListener(
		"keydown",
		(e) => {
			if (e.key === "Escape") dismissShown();
		},
		true,
	);

	const showElement = (msg) => {
		clearShown();
		let nodes = [];
		let invalid = false;
		try {
			nodes = Array.from(document.querySelectorAll(String(msg.selector))).filter(
				(el) => !isOverlay(el),
			);
		} catch (_) {
			nodes = [];
			invalid = true;
		}
		const el = nodes.length === 1 ? nodes[0] : null;
		if (el) {
			shownEl = el;
			el.scrollIntoView({ block: "center", inline: "center" });
			placeShown();
		}
		post({
			type: "shownElement",
			requestId: msg.requestId,
			count: nodes.length,
			element: el ? describe(el) : null,
			...(invalid ? { invalid: true } : {}),
		});
	};

	// ---- scroll position -----------------------------------------------------
	let scrollTimer = null;
	const postScroll = () => {
		scrollTimer = null;
		post({ type: "scroll", x: Math.round(window.scrollX), y: Math.round(window.scrollY) });
	};
	window.addEventListener("scroll", () => {
		if (scrollTimer === null) scrollTimer = setTimeout(postScroll, 200);
	});
	if (document.readyState === "loading") {
		document.addEventListener("DOMContentLoaded", postScroll);
	} else {
		postScroll();
	}

	// ---- hand-over to editor.js (#436) -------------------------------------
	// One-shot: editor.js runs right after this script and deletes it.
	window.__skeinPickerApi = {
		OVERLAY,
		HOST,
		post,
		describe: (el) => describe(el),
		locateOne: (a, all) => locateOne(a, all),
		isOverlay,
		targetAt,
		isPicking: () => picking,
		root: ensureOverlay,
		rectOf,
		rawSourceOf,
		query: safeQuery,
		// Drop pending mutation records so the editor's own writes never
		// read as a page change (which would re-send proposals forever).
		flush: () => {
			try {
				if (mo) mo.takeRecords();
			} catch (_) {
				// No observer yet.
			}
		},
		onSettle: (fn) => settleHooks.push(fn),
	};

	// ---- host messages ---------------------------------------------------

	window.addEventListener("message", (e) => {
		if (e.source !== window.parent) return;
		const m = e.data;
		if (!m || typeof m !== "object" || m.source !== HOST || m.v !== 1) return;
		try {
			switch (m.type) {
				case "pick-start":
					startPick();
					eachKid((w) => sendKid(w, { type: "pick-start" }));
					break;
				case "pick-cancel":
					endPick();
					break;
				case "locate":
					locate(m);
					break;
				case "pins":
					setPins(m);
					break;
				case "highlight":
					highlight(m);
					break;
				case "showElement":
					showElement(m);
					break;
			}
		} catch (_) {
			// A bad host message must never break the page.
		}
	});
})();
