// Touch-mode media emulation for the design preview. Injected at the top of
// <head>, before the prototype's own styles and scripts.
//
// Makes the page answer the `hover`, `any-hover`, `pointer` and `any-pointer`
// media features as a phone does (hover: none, pointer: coarse), for CSS
// media queries and `matchMedia` alike. Nothing on disk or in the server's
// response is rewritten: media query text is rewritten at runtime, in the
// CSSOM, into a condition that evaluates to the emulated answer on THIS host.
//
// Limitations:
//  - Cross-origin stylesheets throw on `cssRules` and are skipped (their
//    own `media` attribute is still rewritten).
//  - Plain `:hover` rules not gated by a media query still follow the native
//    cursor; this only changes what media queries and JS can see.
//  - `<picture><source media>` is not rewritten.
(() => {
	try {
		if (window.__skeinMediaShim) return;
		window.__skeinMediaShim = true;

		const hostMatch = window.matchMedia.bind(window);
		// Emulated value per feature, and the values a feature can take.
		const EMULATED = { hover: "none", pointer: "coarse" };
		const VALUES = { hover: ["none", "hover"], pointer: ["none", "coarse", "fine"] };
		// Always-true / always-false conditions, valid inside `not` and `and`
		// lists. min-color takes a non-negative integer and no display has 99
		// bits per channel; unlike `max-resolution: 0dppx` it is never a
		// parse error, which would turn the whole query into `not all`.
		const TRUE_COND = "(min-width: 0px)";
		const FALSE_COND = "(min-color: 99)";

		const hostSays = (cond) => {
			try {
				return hostMatch(cond).matches;
			} catch {
				return null;
			}
		};

		// Condition text that is `want` on the host for (prefix)feature.
		const cache = new Map();
		const condition = (prefix, feature, want, ownValue) => {
			const key = `${prefix + feature}:${ownValue}:${want}`;
			if (cache.has(key)) return cache.get(key);
			const own = `(${prefix}${feature}: ${ownValue})`;
			let out = want ? TRUE_COND : FALSE_COND;
			if (hostSays(own) === want) out = own;
			else {
				for (const v of VALUES[feature]) {
					const alt = `(${prefix}${feature}: ${v})`;
					if (hostSays(alt) === want) {
						out = alt;
						break;
					}
				}
			}
			cache.set(key, out);
			return out;
		};

		const FEATURE = /\(\s*(any-)?(hover|pointer)\s*(?::\s*([a-z-]+)\s*)?\)/gi;
		const rewrite = (text) =>
			text.replace(FEATURE, (whole, any, feat, val) => {
				const prefix = any ? "any-" : "";
				const feature = feat.toLowerCase();
				if (val === undefined) {
					// Boolean form `(f)` means value != none.
					const truth = EMULATED[feature] !== "none";
					return condition(prefix, feature, truth, "none");
				}
				const value = val.toLowerCase();
				if (!VALUES[feature].includes(value)) return whole;
				return condition(prefix, feature, value === EMULATED[feature], value);
			});

		// MediaList -> { orig, written }: always rewrite from the original so a
		// re-scan is idempotent, but adopt text the page changed in between.
		const seen = new WeakMap();
		const fixMedia = (ml) => {
			if (!ml) return;
			const cur = ml.mediaText;
			let rec = seen.get(ml);
			if (!rec || cur !== rec.written) rec = { orig: cur, written: cur };
			const next = rewrite(rec.orig);
			if (next !== cur) ml.mediaText = next;
			rec.written = ml.mediaText;
			seen.set(ml, rec);
		};

		const walkRules = (rules, visited) => {
			for (const rule of Array.from(rules)) {
				if (rule.media) fixMedia(rule.media);
				if (rule.styleSheet) walkSheet(rule.styleSheet, visited); // @import
				if (rule.cssRules) walkRules(rule.cssRules, visited);
			}
		};
		const walkSheet = (sheet, visited) => {
			if (!sheet || visited.has(sheet)) return;
			visited.add(sheet);
			try {
				fixMedia(sheet.media);
			} catch {}
			let rules;
			try {
				rules = sheet.cssRules; // throws for cross-origin sheets
			} catch {
				return;
			}
			if (rules) walkRules(rules, visited);
		};
		const scan = (only) => {
			try {
				const visited = new WeakSet();
				if (only) return walkSheet(only, visited);
				for (const s of Array.from(document.styleSheets)) walkSheet(s, visited);
				for (const s of document.adoptedStyleSheets || []) walkSheet(s, visited);
			} catch {}
		};

		// Coalesce bursts (a CSS-in-JS library inserts rules one by one).
		let queued = false;
		const scanSoon = () => {
			if (queued) return;
			queued = true;
			Promise.resolve().then(() => {
				queued = false;
				scan();
			});
		};

		// CSS-in-JS (speedy mode inserts thousands of rules): rewrite only the
		// rule that was just inserted, not the whole sheet.
		const wrapInsert = (proto) => {
			const orig = proto?.insertRule;
			if (typeof orig !== "function") return;
			proto.insertRule = function (...args) {
				const index = orig.apply(this, args);
				try {
					const rule = this.cssRules[index];
					if (rule) walkRules([rule], new WeakSet());
				} catch {}
				return index;
			};
		};
		if (typeof CSSStyleSheet !== "undefined") {
			wrapInsert(CSSStyleSheet.prototype);
			const replaceSync = CSSStyleSheet.prototype.replaceSync;
			if (typeof replaceSync === "function") {
				CSSStyleSheet.prototype.replaceSync = function (...args) {
					const result = replaceSync.apply(this, args);
					scan(this);
					return result;
				};
			}
			// replace() resolves later, so rescan everything once it settles.
			const replace = CSSStyleSheet.prototype.replace;
			if (typeof replace === "function") {
				CSSStyleSheet.prototype.replace = function (...args) {
					const p = replace.apply(this, args);
					Promise.resolve(p).then(scanSoon, () => {});
					return p;
				};
			}
		}
		if (typeof CSSGroupingRule !== "undefined") wrapInsert(CSSGroupingRule.prototype);

		const isSheetEl = (n) =>
			n.nodeName === "STYLE" ||
			(n.nodeName === "LINK" && /(^|\s)stylesheet(\s|$)/i.test(n.rel || ""));
		// An added node that is, or contains, a stylesheet element.
		const bringsSheets = (n) => {
			if (n.nodeType !== 1) return false;
			if (isSheetEl(n)) return true;
			return typeof n.querySelector === "function" && !!n.querySelector("style, link");
		};
		const watchLinks = (n) => {
			const links = n.nodeName === "LINK" ? [n] : Array.from(n.querySelectorAll?.("link") || []);
			for (const l of links) l.addEventListener("load", scanSoon);
		};
		if (typeof MutationObserver === "function") {
			new MutationObserver((records) => {
				for (const r of records) {
					// Text edits count only inside a <style> (React text updates don't).
					if (r.type === "characterData") {
						if (r.target.parentNode?.nodeName === "STYLE") return scanSoon();
						continue;
					}
					// `style.textContent = css` replaces the STYLE's children.
					if (r.target?.nodeName === "STYLE") return scanSoon();
					for (const n of Array.from(r.addedNodes || [])) {
						if (bringsSheets(n)) {
							watchLinks(n);
							return scanSoon();
						}
					}
				}
			}).observe(document.documentElement, {
				childList: true,
				subtree: true,
				characterData: true,
			});
		}

		window.matchMedia = function matchMedia(query) {
			const original = String(query);
			const mql = hostMatch(rewrite(original));
			try {
				// Callers must read back the query they wrote.
				Object.defineProperty(mql, "media", { value: original, configurable: true });
			} catch {}
			return mql;
		};

		scan();
		document.addEventListener("DOMContentLoaded", scan);
		window.addEventListener("load", scan);
	} catch {}
})();
