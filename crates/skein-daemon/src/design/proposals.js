// Skein design preview: pending-proposal overlay (#436), the LAST injected
// script (after picker.js and editor.js). The host sends `proposals` for the
// open proposal threads; each one is located, its changes are previewed with
// inline styles and a dashed "proposed" outline is drawn. Every message
// clears and redraws, so an item the host drops disappears.
// It takes its helpers from `api.editor` (editor.js) and deletes the one-shot
// `window.__skeinPickerApi` that picker.js left for both scripts.
(() => {
	const api = window.__skeinPickerApi;
	try {
		delete window.__skeinPickerApi;
	} catch (_) {
		window.__skeinPickerApi = undefined;
	}
	const ed = api?.editor;
	if (!ed) return;
	const { OVERLAY, HOST, isOverlay, root, rectOf, query, flush } = api;
	const { ALLOWLIST, MAX_TEXT, cleanVal, textOnly } = ed;

	const previews = new Map(); // element -> { style, text }
	let outlines = [];
	let lastItems = null;

	const clearPreviews = () => {
		for (const [e, s] of previews) {
			if (s.style === null) e.removeAttribute("style");
			else e.setAttribute("style", s.style);
			if (s.text !== null && e.textContent !== s.text) e.textContent = s.text;
		}
		previews.clear();
		for (const n of outlines) n.remove();
		outlines = [];
	};

	const find = (a, all) => {
		const f = api.locateOne(a, all);
		const tag = typeof a.tag === "string" ? a.tag.toLowerCase() : "";
		const sel = f.bySelector;
		const selOk = sel && sel.tag === tag;
		const cand =
			(selOk && sel.text === (a.text || "") ? sel : null) ||
			f.byOdId[0] ||
			f.byText[0] ||
			(selOk ? sel : null);
		return cand ? query(cand.selector) : null;
	};

	const previewChanges = (e, changes) => {
		if (!previews.has(e)) {
			previews.set(e, {
				style: e.getAttribute("style"),
				text: textOnly(e) ? e.textContent : null,
			});
		}
		const base = previews.get(e);
		for (const c of changes) {
			if (!c || typeof c !== "object") continue;
			if (c.kind === "style" && ALLOWLIST.includes(c.property)) {
				const v = cleanVal(c.to);
				if (v) e.style.setProperty(c.property, v);
			} else if (c.kind === "offset" && Number.isFinite(c.dx) && Number.isFinite(c.dy)) {
				const t = e.style.transform;
				e.style.transform = `translate(${c.dx}px, ${c.dy}px) ${t}`.trim();
			} else if (
				c.kind === "text" &&
				base.text !== null &&
				typeof c.to === "string" &&
				c.to.length <= MAX_TEXT
			) {
				e.textContent = c.to;
			}
		}
	};

	const outline = (e) => {
		const r = rectOf(e);
		const node = document.createElement("div");
		node.setAttribute(OVERLAY, "proposal");
		node.style.cssText = `position:absolute;left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px;box-sizing:border-box;border:2px dashed #8a5cf5;pointer-events:none;`;
		const label = document.createElement("span");
		label.textContent = "proposed";
		label.style.cssText =
			"position:absolute;left:-2px;top:-18px;padding:0 4px;background:#8a5cf5;color:#fff;font:600 10px/16px system-ui,sans-serif;";
		node.appendChild(label);
		root().appendChild(node);
		outlines.push(node);
	};

	const show = (items) => {
		lastItems = items;
		if (ed.active()) return; // redrawn when edit mode ends
		clearPreviews();
		const all = Array.from(document.querySelectorAll("*")).filter((x) => !isOverlay(x));
		for (const it of items.slice(0, 50)) {
			if (!it || typeof it !== "object" || !it.anchor || !Array.isArray(it.changes)) continue;
			try {
				const e = find(it.anchor, all);
				if (!e) continue;
				previewChanges(e, it.changes);
				outline(e);
			} catch (_) {
				// One bad item must not drop the others.
			}
		}
		flush();
	};

	const redraw = () => {
		if (lastItems) show(lastItems);
	};
	// The page may re-render and wipe the inline previews; picker.js calls
	// this each time the DOM settles.
	api.onSettle(redraw);
	ed.onStart(() => {
		clearPreviews();
		flush();
	});
	ed.onEnd(redraw);

	window.addEventListener("message", (e) => {
		if (e.source !== window.parent) return;
		const m = e.data;
		if (!m || typeof m !== "object" || m.source !== HOST || m.v !== 1) return;
		try {
			if (m.type === "proposals") show(Array.isArray(m.items) ? m.items : []);
		} catch (_) {
			// A bad host message must never break the page.
		}
	});
})();
