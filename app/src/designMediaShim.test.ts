import { describe, expect, it } from "vitest";
import SRC from "../src-tauri/src/design/media.js?raw";

type Sheet = {
	media: { mediaText: string };
	cssRules: { media: { mediaText: string }; cssRules: unknown[] }[];
};

// Host answers: the set of exact queries that are true on it.
function load(trueQueries: string[], sheets: Sheet[] = []) {
	const received: string[] = [];
	const observers: ((records: unknown[]) => void)[] = [];
	const sandbox: Record<string, unknown> = {
		matchMedia: (q: string) => {
			received.push(q);
			return { matches: trueQueries.includes(q), media: q };
		},
		document: {
			styleSheets: sheets,
			documentElement: {},
			readyState: "loading",
			addEventListener: () => {},
		},
		MutationObserver: class {
			constructor(cb: (records: unknown[]) => void) {
				observers.push(cb);
			}
			observe() {}
		},
		addEventListener: () => {},
	};
	sandbox.window = sandbox;
	// The script reads `window`, `document` and `MutationObserver` as free names.
	new Function("window", "document", "MutationObserver", SRC)(
		sandbox,
		sandbox.document,
		sandbox.MutationObserver,
	);
	const mm = sandbox.matchMedia as (q: string) => { matches: boolean; media: string };
	// What the host saw for the final (rewritten) call, and its answer.
	const ask = (q: string) => {
		received.length = 0;
		const mql = mm(q);
		return { mql, host: received[received.length - 1] ?? "" };
	};
	return { ask, received, observers };
}

const DESKTOP = ["(hover: hover)", "(any-hover: hover)", "(pointer: fine)", "(any-pointer: fine)"];

describe("design media shim", () => {
	const { ask } = load(DESKTOP);

	it("flips hover/pointer to touch values", () => {
		expect(ask("(hover: hover)").mql.matches).toBe(false);
		expect(ask("(hover: none)").mql.matches).toBe(true);
		expect(ask("(pointer: coarse)").mql.matches).toBe(true);
		expect(ask("(pointer: fine)").mql.matches).toBe(false);
		expect(ask("(any-hover: hover)").mql.matches).toBe(false);
		expect(ask("(any-pointer: coarse)").mql.matches).toBe(true);
	});

	it("treats the boolean form as value != none", () => {
		expect(ask("(hover)").mql.matches).toBe(false);
		expect(ask("(pointer)").mql.matches).toBe(true);
	});

	it("is case and whitespace tolerant", () => {
		expect(ask("( HOVER :  Hover )").mql.matches).toBe(false);
	});

	it("keeps the rest of the query", () => {
		const { host } = ask("(min-width: 600px) and (hover: hover)");
		expect(host.startsWith("(min-width: 600px) and ")).toBe(true);
		expect(host).not.toContain("(hover: hover)");
		expect(ask("not all and (hover: hover)").host).not.toContain("(hover: hover)");
	});

	it("passes unrelated and unknown queries through", () => {
		expect(ask("(prefers-color-scheme: dark)").host).toBe("(prefers-color-scheme: dark)");
		expect(ask("(hover: bogus)").host).toBe("(hover: bogus)");
	});

	it("reads back the caller's query as media", () => {
		expect(ask("(hover: hover)").mql.media).toBe("(hover: hover)");
	});

	it("substitutes on a touch-capable host (coarse and fine both true)", () => {
		const hybrid = [...DESKTOP, "(any-pointer: coarse)", "(any-hover: none)"];
		const { ask: a } = load(hybrid);
		// want any-pointer:fine false -> some other value that is false on host
		expect(a("(any-pointer: fine)").host).toBe("(any-pointer: none)");
		// want any-pointer:coarse true -> own text kept
		expect(a("(any-pointer: coarse)").host).toBe("(any-pointer: coarse)");
		// want any-pointer:none false -> own text kept (false on host)
		expect(a("(any-pointer: none)").host).toBe("(any-pointer: none)");
	});

	it("falls back to fixed conditions when no value matches", () => {
		const { ask: a } = load(["(hover: hover)", "(hover: none)"]);
		// want hover:hover false; both values are true on this host
		expect(a("(hover: hover)").host).toBe("(min-color: 99)");
		// want hover:none true; own text is true on the host
		expect(a("(hover: none)").host).toBe("(hover: none)");
	});

	it("rewrites stylesheets at startup, idempotently", () => {
		const rule = { media: { mediaText: "(hover: hover)" }, cssRules: [] };
		const sheet: Sheet = { media: { mediaText: "" }, cssRules: [rule] };
		const { ask: a } = load(DESKTOP, [sheet]);
		expect(rule.media.mediaText).not.toBe("(hover: hover)");
		const after = rule.media.mediaText;
		// Later matchMedia traffic must not disturb the rewritten rule.
		a("(hover)");
		expect(rule.media.mediaText).toBe(after);
	});

	it("rescans on a STYLE textContent change but not on unrelated text", async () => {
		const rule = { media: { mediaText: "" }, cssRules: [] };
		const sheet: Sheet = { media: { mediaText: "" }, cssRules: [rule] };
		const { observers } = load(DESKTOP, [sheet]);
		const observer = observers[0];
		if (!observer) throw new Error("no observer");
		rule.media.mediaText = "(hover: hover)";
		// A React-style text update outside any <style>: no rescan.
		observer([{ type: "characterData", target: { parentNode: { nodeName: "DIV" } } }]);
		await Promise.resolve();
		expect(rule.media.mediaText).toBe("(hover: hover)");
		// style.textContent = css: childList record targeting the STYLE.
		observer([{ type: "childList", target: { nodeName: "STYLE" }, addedNodes: [{ nodeType: 3 }] }]);
		await Promise.resolve();
		await Promise.resolve();
		expect(rule.media.mediaText).not.toBe("(hover: hover)");
	});

	it("insertRule rewrites only the inserted rule", () => {
		const g = globalThis as Record<string, unknown>;
		const untouched = { media: { mediaText: "(hover: hover)" }, cssRules: [] };
		const inserted = { media: { mediaText: "(hover: hover)" }, cssRules: [] };
		class FakeSheet {
			cssRules = [untouched];
			insertRule(_text: string, index = 0) {
				this.cssRules.splice(index, 0, inserted);
				return index;
			}
		}
		g.CSSStyleSheet = FakeSheet;
		try {
			load(DESKTOP);
			const sheet = new FakeSheet();
			sheet.insertRule("@media (hover: hover) {}", 1);
			expect(inserted.media.mediaText).not.toBe("(hover: hover)");
			expect(untouched.media.mediaText).toBe("(hover: hover)");
		} finally {
			Reflect.deleteProperty(g, "CSSStyleSheet");
		}
	});
});
