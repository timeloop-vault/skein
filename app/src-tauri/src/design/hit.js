// Pure hit-testing helpers for the element picker. No DOM access in here:
// the picker gathers rects and attributes, these decide. Classic script.
(() => {
	const area = (r) => Math.max(0, r.width) * Math.max(0, r.height);

	// Index of the deepest candidate containing the point, or -1.
	// Deeper wins; then smaller area; then later in document order.
	const pickDeepest = (point, candidates) => {
		let best = -1;
		if (!point || !Array.isArray(candidates)) return best;
		for (let i = 0; i < candidates.length; i++) {
			const c = candidates[i];
			const r = c?.rect;
			if (!r || !(r.width > 0) || !(r.height > 0)) continue;
			if (point.x < r.left || point.x > r.left + r.width) continue;
			if (point.y < r.top || point.y > r.top + r.height) continue;
			if (best < 0) {
				best = i;
				continue;
			}
			const b = candidates[best];
			if (c.depth !== b.depth) {
				if (c.depth > b.depth) best = i;
				continue;
			}
			const ca = area(r);
			const ba = area(b.rect);
			if (ca !== ba) {
				if (ca < ba) best = i;
				continue;
			}
			if ((c.order ?? 0) >= (b.order ?? 0)) best = i;
		}
		return best;
	};

	const clip = (s, n) => {
		const t = String(s).replace(/\s+/g, " ").trim();
		return t.length > n ? `${t.slice(0, n - 1)}…` : t;
	};

	// `tag · role · "label"`; the label is aria-label, else text, clipped.
	const labelOf = (info) => {
		const i = info || {};
		const parts = [i.tag || "element"];
		if (i.role) parts.push(String(i.role));
		const text = i.ariaLabel || i.text;
		if (text && String(text).trim()) parts.push(`"${clip(text, 40)}"`);
		return parts.join(" · ");
	};

	// Flutter web numbers semantics nodes by layout; they change on rebuild.
	const isUnstableId = (id) => /^flt-semantic-node-\d+$/.test(String(id ?? ""));

	// The pick is never coarser than the browser's own hit. The rect winner
	// may replace a whole-page host target, or refine downward to a
	// descendant of the hit; anything else keeps the hit.
	const refine = (hit, rectWinner, { isHost, contains }) => {
		if (isHost(hit)) return rectWinner ?? hit;
		if (rectWinner && rectWinner !== hit && contains(hit, rectWinner)) return rectWinner;
		return hit;
	};

	window.__skeinHit = { pickDeepest, labelOf, isUnstableId, refine };
})();
