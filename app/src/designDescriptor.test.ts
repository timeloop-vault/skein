import { describe, expect, it } from "vitest";
import { stripSourcePrefix } from "./designDescriptor.ts";

describe("stripSourcePrefix", () => {
	it("maps preview URLs to worktree paths", () => {
		expect(stripSourcePrefix("/http:/127.0.0.1:1234/preview/tok/proto/shell.jsx")).toBe(
			"proto/shell.jsx",
		);
		expect(stripSourcePrefix("http://127.0.0.1:1/preview/tok/a/b/c.html?v=2")).toBe("a/b/c.html");
		expect(stripSourcePrefix("/preview/tok/my%20dir/f%C3%A4.jsx")).toBe("my dir/fä.jsx");
	});
	it("refuses traversal, backslashes, empties and non-preview names", () => {
		expect(stripSourcePrefix("/preview/tok/../x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a/%2e%2e/x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%5Cb.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%2Fb.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/C%3A/x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok")).toBeUndefined();
		expect(stripSourcePrefix("/preview//x.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a//b.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/%E0%A4%A.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a/b:c.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%3Ab.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/a%00b.js")).toBeUndefined();
		expect(stripSourcePrefix("/preview/tok/My%20Proto.jsx")).toBe("My Proto.jsx");
		expect(stripSourcePrefix(`/preview/tok/${"a".repeat(501)}`)).toBeUndefined();
		expect(stripSourcePrefix(`/preview/tok/${"a".repeat(500)}`)).toBe("a".repeat(500));
		expect(stripSourcePrefix("/Inline Babel script")).toBeUndefined();
		expect(stripSourcePrefix("http://cdn.example/react.js")).toBeUndefined();
	});
});
