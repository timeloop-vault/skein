// Skein design preview: the script injected into every served HTML page.
// It only reports upward; #434 extends this with selection/anchoring.
(() => {
	const SOURCE = "skein-design";
	const MAX = 500;
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
})();
