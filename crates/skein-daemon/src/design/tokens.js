// Skein design preview: design tokens (#436), injected between picker.js and
// editor.js. Finds the CSS custom properties declared on :root/html and
// compares values the way a designer would (colours resolved by the browser),
// so editor.js can name the token a new value matches. Adds `api.tokens` to
// the one-shot `window.__skeinPickerApi`, which proposals.js finally deletes.
(() => {
	const api = window.__skeinPickerApi;
	if (!api) return;
	const { OVERLAY, root } = api;
	const MAX_VAL = 200;
	const MAX_TOKENS = 200;
	// Keep in step with the colour entries of ALLOWLIST in editor.js.
	const COLOR_PROPS = ["color", "background-color", "border-color"];
	const norm = (s) => String(s).trim().toLowerCase().replace(/\s+/g, " ");

	const scanRules = (rules, out, depth) => {
		for (const rule of Array.from(rules)) {
			if (rule.style && typeof rule.selectorText === "string") {
				const rootRule = rule.selectorText.split(",").some((s) => /^\s*(:root|html)\s*$/i.test(s));
				if (!rootRule) continue;
				for (const name of Array.from(rule.style)) {
					if (!name.startsWith("--")) continue;
					out.set(name, rule.style.getPropertyValue(name).trim().slice(0, MAX_VAL));
				}
			} else if (rule.cssRules && depth < 2) {
				scanRules(rule.cssRules, out, depth + 1);
			}
		}
	};
	const scanTokens = () => {
		const out = new Map();
		for (const sheet of Array.from(document.styleSheets)) {
			try {
				scanRules(sheet.cssRules, out, 0);
			} catch (_) {
				// Cross-origin sheet: its rules are unreadable.
			}
		}
		return Array.from(out, ([name, value]) => ({ name, value })).slice(0, MAX_TOKENS);
	};

	let probe = null;
	const resolveColor = (v) => {
		try {
			if (!probe?.isConnected) {
				probe = document.createElement("span");
				probe.setAttribute(OVERLAY, "probe");
				probe.style.cssText = "display:none;";
				root().appendChild(probe);
			}
			probe.style.color = "";
			probe.style.color = v;
			return probe.style.color ? getComputedStyle(probe).color : norm(v);
		} catch (_) {
			return norm(v);
		}
	};
	const sameValue = (prop, a, b) =>
		COLOR_PROPS.includes(prop) ? resolveColor(a) === resolveColor(b) : norm(a) === norm(b);

	api.tokens = { scanTokens, sameValue };
})();
