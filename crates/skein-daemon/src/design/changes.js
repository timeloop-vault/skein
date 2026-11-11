// Skein design preview: show_changes (#547). Injected after picker.js and
// editor.js, before proposals.js (which deletes the one-shot
// `window.__skeinPickerApi` this reads). Two host messages: `listSources`
// reports where the rendered elements come from in the JSX (React's own
// per-element _debugSource, grouped per opening tag), and `showChanges`
// outlines every element made at the given sites.
(() => {
	const api = window.__skeinPickerApi;
	if (!api || typeof api.post !== "function" || typeof api.rawSourceOf !== "function") return;
	const { HOST, OVERLAY, post, isOverlay, rectOf, rawSourceOf, root, onSettle } = api;

	const allElements = () =>
		Array.from(document.querySelectorAll("*")).filter((el) => !isOverlay(el));

	const onScreen = (el) => {
		const r = el.getBoundingClientRect();
		if (r.width <= 0 || r.height <= 0) return false;
		return r.right > 0 && r.bottom > 0 && r.left < window.innerWidth && r.top < window.innerHeight;
	};

	const MAX_SITES = 5000;
	const siteKey = (fileName, line, endLine) => `${fileName}\u0000${line}\u0000${endLine}`;

	const listSources = (m) => {
		const groups = new Map();
		let capped = false;
		for (const el of allElements()) {
			const src = rawSourceOf(el);
			if (!src || !Number.isFinite(src.lineNumber)) continue;
			const endLine = Number.isFinite(src.endLineNumber) ? src.endLineNumber : src.lineNumber;
			const key = siteKey(src.fileName, src.lineNumber, endLine);
			let g = groups.get(key);
			if (!g) {
				if (groups.size >= MAX_SITES) {
					capped = true;
					continue;
				}
				g = { fileName: src.fileName, line: src.lineNumber, endLine, count: 0, onScreen: 0 };
				groups.set(key, g);
			}
			g.count += 1;
			if (onScreen(el)) g.onScreen += 1;
		}
		post({
			type: "sources",
			requestId: m.requestId,
			sites: Array.from(groups.values()),
			...(capped ? { capped: true } : {}),
		});
	};

	let outlined = []; // [{ el, box }]
	const clear = () => {
		for (const o of outlined) if (o.box) o.box.remove();
		outlined = [];
	};
	const dismiss = () => {
		if (outlined.length === 0) return;
		clear();
		post({ type: "changesCleared" });
	};
	const place = () => {
		if (outlined.length === 0) return;
		const host = root();
		for (const o of outlined) {
			if (!o.el.isConnected) {
				if (o.box) o.box.remove();
				o.box = null;
				continue;
			}
			const r = rectOf(o.el);
			if (r.w <= 0 || r.h <= 0) {
				if (o.box) o.box.style.display = "none";
				continue;
			}
			if (!o.box || o.box.parentNode !== host) {
				if (o.box) o.box.remove();
				o.box = document.createElement("div");
				o.box.setAttribute(OVERLAY, "changed");
				o.box.style.cssText =
					"position:absolute;box-sizing:border-box;border:2px dashed #e5329b;background:rgba(229,50,155,.08);pointer-events:none;";
				host.appendChild(o.box);
			}
			o.box.style.display = "";
			o.box.style.left = `${r.x}px`;
			o.box.style.top = `${r.y}px`;
			o.box.style.width = `${r.w}px`;
			o.box.style.height = `${r.h}px`;
		}
	};
	if (typeof onSettle === "function") onSettle(place);
	window.addEventListener("resize", place);
	document.addEventListener("click", dismiss, true);
	document.addEventListener(
		"keydown",
		(e) => {
			if (e.key === "Escape") dismiss();
		},
		true,
	);

	const showChanges = (m) => {
		clear();
		const want = new Set();
		if (Array.isArray(m.sites)) {
			for (const s of m.sites) {
				if (s && typeof s.fileName === "string") {
					want.add(siteKey(s.fileName, s.line, Number.isFinite(s.endLine) ? s.endLine : s.line));
				}
			}
		}
		const max = Number.isFinite(m.max) && m.max > 0 ? Math.floor(m.max) : 200;
		const hits = [];
		if (want.size > 0) {
			for (const el of allElements()) {
				const src = rawSourceOf(el);
				if (!src) continue;
				const end = Number.isFinite(src.endLineNumber) ? src.endLineNumber : src.lineNumber;
				if (want.has(siteKey(src.fileName, src.lineNumber, end))) hits.push(el);
			}
		}
		const capped = hits.length > max;
		outlined = hits.slice(0, max).map((el) => ({ el, box: null }));
		const first = outlined.find((o) => {
			const r = o.el.getBoundingClientRect();
			return r.width > 0 && r.height > 0;
		});
		if (first) first.el.scrollIntoView({ block: "nearest", inline: "nearest" });
		place();
		post({
			type: "shownChanges",
			requestId: m.requestId,
			highlighted: outlined.length,
			...(capped ? { capped: true } : {}),
		});
	};

	window.addEventListener("message", (e) => {
		if (e.source !== window.parent) return;
		const m = e.data;
		if (!m || typeof m !== "object" || m.source !== HOST || m.v !== 1) return;
		try {
			if (m.type === "listSources") listSources(m);
			else if (m.type === "showChanges") showChanges(m);
		} catch (_) {
			// A bad host message must never break the page.
		}
	});
})();
