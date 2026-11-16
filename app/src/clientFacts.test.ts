import { describe, expect, it } from "vitest";
import { clientFacts, createFactsDiffer, createReportCounter, sameFacts } from "./clientFacts.ts";
import { CLEAN_DRAFT } from "./composerDraft.ts";

describe("clientFacts", () => {
	it("reports a clean draft as not held, with no reason", () => {
		expect(clientFacts(CLEAN_DRAFT, 3)).toEqual({ draft: { held: false }, userInputSeq: 3 });
	});

	it.each([[{ kind: "typed", chars: 2 } as const], [{ kind: "unknown" } as const]])(
		"reports %j as held with checkDraft's refusal text",
		(draft) => {
			const facts = clientFacts(draft, 0);
			expect(facts.draft.held).toBe(true);
			expect(facts.draft.held && facts.draft.reason).toMatch(/composer holds unsent text/);
		},
	);
});

describe("sameFacts", () => {
	it("compares held, reason and the counter", () => {
		const held = clientFacts({ kind: "typed", chars: 1 }, 1);
		expect(sameFacts(held, clientFacts({ kind: "unknown" }, 1))).toBe(true);
		expect(sameFacts(held, clientFacts({ kind: "typed", chars: 1 }, 2))).toBe(false);
		expect(sameFacts(held, clientFacts(CLEAN_DRAFT, 1))).toBe(false);
	});
});

describe("createFactsDiffer", () => {
	const clean = (n: number) => clientFacts(CLEAN_DRAFT, n);

	it("sends the first report and skips an identical one", () => {
		const d = createFactsDiffer();
		expect(d.next("h", clean(0))).toEqual(clean(0));
		expect(d.next("h", clean(0))).toBeNull();
	});

	it("sends when the draft or the counter changed", () => {
		const d = createFactsDiffer();
		d.next("h", clean(0));
		expect(d.next("h", clean(1))).toEqual(clean(1));
		const held = clientFacts({ kind: "typed", chars: 1 }, 1);
		expect(d.next("h", held)).toEqual(held);
		expect(d.next("h", held)).toBeNull();
	});

	it("tracks harnesses independently", () => {
		const d = createFactsDiffer();
		d.next("a", clean(0));
		expect(d.next("b", clean(0))).not.toBeNull();
	});

	it("force sends an identical report (a terminal registering)", () => {
		const d = createFactsDiffer();
		d.next("h", clean(0));
		expect(d.next("h", clean(0), true)).toEqual(clean(0));
		expect(d.next("h", clean(0))).toBeNull();
	});

	it("forget makes the next report go out", () => {
		const d = createFactsDiffer();
		d.next("h", clean(0));
		d.forget("h");
		expect(d.next("h", clean(0))).not.toBeNull();
	});
});

describe("createReportCounter", () => {
	it("rises with every call, from the seed", () => {
		const next = createReportCounter(100);
		expect([next(), next(), next()]).toEqual([101, 102, 103]);
	});

	it("a counter made later (a reloaded page) starts above an earlier one", () => {
		const first = createReportCounter(1_000);
		for (let i = 0; i < 50; i++) first();
		expect(createReportCounter(2_000)()).toBeGreaterThan(first());
	});

	it("defaults to the clock", () => {
		const before = Date.now();
		expect(createReportCounter()()).toBeGreaterThan(before);
	});
});
