import { describe, expect, it } from "vitest";
import { describeEffective, formatFailure, type UpdaterDiagnosisDto } from "./updaterProxy.ts";

describe("describeEffective", () => {
	it("names the Settings source", () => {
		expect(
			describeEffective({
				source: "setting",
				url: "http://127.0.0.1:3128/",
				noProxy: null,
				bypassedByNoProxy: false,
				host: null,
				skipped: [],
			}),
		).toBe("Using http://127.0.0.1:3128/ from Settings");
	});
	it("names an env var", () => {
		expect(
			describeEffective({
				source: "env:HTTPS_PROXY",
				url: "http://p:1/",
				noProxy: null,
				bypassedByNoProxy: false,
				host: null,
				skipped: [],
			}),
		).toBe("Using http://p:1/ from $HTTPS_PROXY");
	});
	it("says when NO_PROXY skips the proxy", () => {
		expect(
			describeEffective({
				source: "env:HTTPS_PROXY",
				url: "http://p:1/",
				noProxy: "github.com",
				bypassedByNoProxy: true,
				host: "github.com",
				skipped: [],
			}),
		).toContain("skipped for github.com because of NO_PROXY");
	});
	it("names the host and skipped values", () => {
		const out = describeEffective({
			source: "env:HTTPS_PROXY",
			url: "http://p:1/",
			noProxy: "x",
			bypassedByNoProxy: true,
			host: "example.org",
			skipped: ["a", "b"],
		});
		expect(out).toContain("skipped for example.org because");
		expect(out).toContain("ignored invalid: a, b");
	});
	it("explains the system fallback", () => {
		expect(
			describeEffective({
				source: "system",
				url: null,
				noProxy: null,
				bypassedByNoProxy: false,
				host: null,
				skipped: [],
			}),
		).toContain("PAC files are not evaluated");
	});
});

describe("formatFailure", () => {
	const d = (chain: string[]): UpdaterDiagnosisDto => ({
		url: "https://x",
		effective: {
			source: "setting",
			url: "http://p:1/",
			noProxy: null,
			bypassedByNoProxy: false,
			host: null,
			skipped: [],
		},
		ok: false,
		status: null,
		chain,
	});
	it("returns only the message without a diagnosis", () => {
		expect(formatFailure("boom", null)).toEqual(["Update failed: boom"]);
	});
	it("adds cause and proxy lines", () => {
		expect(formatFailure("boom", d(["a", "b"]))).toEqual([
			"Update failed: boom",
			"Cause: a → b",
			"Using http://p:1/ from Settings",
		]);
	});
	it("omits an empty chain", () => {
		expect(formatFailure("boom", d([]))).toHaveLength(2);
	});
});
