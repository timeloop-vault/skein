import { describe, expect, it, vi } from "vitest";
import {
	pickDesignHarness,
	publishDesignFocus,
	subscribeDesignFocus,
	takeDesignFocus,
} from "./designFocus.ts";

const f = (harnessId: string, threadId = "t") => ({
	roomId: "r",
	harnessId,
	entry: "a.html",
	threadId,
});

describe("design focus", () => {
	it("stays pending when nobody is subscribed", () => {
		publishDesignFocus(f("h1"));
		expect(takeDesignFocus("h1")?.threadId).toBe("t");
		expect(takeDesignFocus("h1")).toBeUndefined();
	});

	it("is not left pending once a subscriber heard it", () => {
		const heard: string[] = [];
		const off = subscribeDesignFocus("h2", (x) => heard.push(x.threadId));
		publishDesignFocus(f("h2"));
		off();
		expect(heard).toEqual(["t"]);
		expect(takeDesignFocus("h2")).toBeUndefined();
	});

	it("notifies subscribers and stops after unsubscribe", () => {
		const cb = vi.fn();
		const off = subscribeDesignFocus("h3", cb);
		publishDesignFocus(f("h3"));
		expect(cb).toHaveBeenCalledTimes(1);
		off();
		publishDesignFocus(f("h3", "t2"));
		expect(cb).toHaveBeenCalledTimes(1);
		expect(takeDesignFocus("h3")?.threadId).toBe("t2");
	});

	it("keeps only the latest pending request per harness", () => {
		publishDesignFocus(f("h4", "old"));
		publishDesignFocus(f("h4", "new"));
		publishDesignFocus(f("h5"));
		expect(takeDesignFocus("h4")?.threadId).toBe("new");
		expect(takeDesignFocus("h4")).toBeUndefined();
		expect(takeDesignFocus("h5")?.threadId).toBe("t");
	});
});

describe("pickDesignHarness", () => {
	const h = (id: string, kind: "design" | "claude", designEntry?: string) => ({
		id,
		kind,
		...(designEntry ? { designEntry } : {}),
	});
	it("prefers the harness already on the entry", () => {
		const room = { harnesses: [h("a", "design", "x.html"), h("b", "design", "a.html")] };
		expect(pickDesignHarness(room, "a.html")).toEqual({ harnessId: "b", setEntry: false });
	});
	it("falls back to the first design harness", () => {
		const room = { harnesses: [h("c", "claude"), h("a", "design", "x.html"), h("b", "design")] };
		expect(pickDesignHarness(room, "a.html")).toEqual({ harnessId: "a", setEntry: true });
	});
	it("is null without a design harness", () => {
		expect(pickDesignHarness({ harnesses: [h("c", "claude")] }, "a.html")).toBeNull();
	});
});
