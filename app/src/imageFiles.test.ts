import { describe, expect, it } from "vitest";
import {
	formatBytes,
	imageMime,
	isImagePath,
	MAX_IMAGE_BYTES,
	parseImageError,
} from "./imageFiles.ts";

describe("imageMime", () => {
	it.each([
		["photo.png", "image/png"],
		["photo.PNG", "image/png"],
		["photo.jpg", "image/jpeg"],
		["photo.JPEG", "image/jpeg"],
		["anim.gif", "image/gif"],
		["pic.webp", "image/webp"],
		["icon.svg", "image/svg+xml"],
		["notes.txt", null],
		["README", null],
		["archive.tar.gz", null],
		["weird.pngx", null],
	])("%s -> %s", (path, want) => {
		expect(imageMime(path)).toBe(want);
		expect(isImagePath(path)).toBe(want != null);
	});

	it("works on both path separators", () => {
		expect(imageMime("src/assets/logo.png")).toBe("image/png");
		expect(imageMime("src\\assets\\logo.png")).toBe("image/png");
		expect(imageMime("C:\\Users\\me\\shot.webp")).toBe("image/webp");
	});

	it("treats a dotfile with no stem as not an image", () => {
		expect(imageMime(".png")).toBeNull();
		expect(imageMime("dir/.png")).toBeNull();
	});

	it("treats a trailing dot with no extension as not an image", () => {
		expect(imageMime("weird.")).toBeNull();
		expect(imageMime("a.")).toBeNull();
	});
});

describe("MAX_IMAGE_BYTES", () => {
	it("is 16 MiB", () => {
		expect(MAX_IMAGE_BYTES).toBe(16 * 1024 * 1024);
	});
});

describe("parseImageError", () => {
	it("parses toolarge with the byte length", () => {
		expect(parseImageError("toolarge:12345")).toEqual({ kind: "toolarge", len: 12345 });
	});

	it("parses notimage", () => {
		expect(parseImageError("notimage")).toEqual({ kind: "notimage" });
	});

	it("parses missing and unavailable", () => {
		expect(parseImageError("missing")).toEqual({ kind: "missing" });
		expect(parseImageError("unavailable")).toEqual({ kind: "unavailable" });
	});

	it("falls back to other with the raw message", () => {
		expect(parseImageError("path is outside every room's folder")).toEqual({
			kind: "other",
			msg: "path is outside every room's folder",
		});
	});
});

describe("formatBytes", () => {
	it.each([
		[0, "0 B"],
		[512, "512 B"],
		[1023, "1023 B"],
		[1024, "1.0 KB"],
		[3482, "3.4 KB"],
		[1024 * 1024 - 1, "1024.0 KB"],
		[1024 * 1024, "1.0 MB"],
		[16 * 1024 * 1024, "16.0 MB"],
	])("%i -> %s", (n, want) => {
		expect(formatBytes(n)).toBe(want);
	});
});
