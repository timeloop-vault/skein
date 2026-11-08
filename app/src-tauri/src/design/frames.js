// Skein design preview: pure helpers for relaying picks and locates through
// child iframes (#708). No DOM in here; picker.js supplies the measurements.
// A "box" is the iframe's content-box origin in its PARENT document's
// coordinates; "childScroll" is the child document's current {x, y} scroll.
(() => {
	const num = (n) => (typeof n === "number" && Number.isFinite(n) ? n : 0);

	// Child document rect -> parent document rect.
	const toParentRect = (rect, box, childScroll) => {
		const r = rect || {};
		const b = box || {};
		const s = childScroll || {};
		return {
			x: Math.round(num(r.x) - num(s.x) + num(b.x)),
			y: Math.round(num(r.y) - num(s.y) + num(b.y)),
			w: num(r.w),
			h: num(r.h),
		};
	};

	const prefixDescriptor = (d, frameSel, box, childScroll) => {
		if (!d || typeof d !== "object") return d;
		const out = { ...d, frame: [frameSel, ...(Array.isArray(d.frame) ? d.frame : [])] };
		if (d.rect) out.rect = toParentRect(d.rect, box, childScroll);
		return out;
	};

	// Split anchors into those for this document and those for child frames,
	// keyed by frame[0]; the frame path handed down is the rest.
	const partitionAnchors = (anchors) => {
		const local = [];
		const groups = new Map();
		for (const a of Array.isArray(anchors) ? anchors : []) {
			if (!a || typeof a !== "object") continue;
			const f = Array.isArray(a.frame) ? a.frame : [];
			if (f.length === 0 || typeof f[0] !== "string") {
				local.push(a);
				continue;
			}
			const { frame: _drop, ...rest } = a;
			const sub = f.slice(1);
			if (sub.length) rest.frame = sub;
			let g = groups.get(f[0]);
			if (!g) {
				g = { sel: f[0], anchors: [] };
				groups.set(f[0], g);
			}
			g.anchors.push(rest);
		}
		return { local, byFrame: Array.from(groups.values()) };
	};

	const emptyFound = () => ({ bySelector: null, byOdId: [], byText: [], sameTag: [] });

	const relayLocated = (found, frameSel, box, childScroll) => {
		const f = found || emptyFound();
		const one = (d) => prefixDescriptor(d, frameSel, box, childScroll);
		const many = (l) => (Array.isArray(l) ? l.map(one) : []);
		return {
			bySelector: f.bySelector ? one(f.bySelector) : null,
			byOdId: many(f.byOdId),
			byText: many(f.byText),
			sameTag: many(f.sameTag),
		};
	};

	window.__skeinFrames = {
		toParentRect,
		prefixDescriptor,
		partitionAnchors,
		relayLocated,
		emptyFound,
	};
})();
