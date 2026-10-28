import { describe, expect, it } from "vitest";
import { contextHarnessIds } from "./contextHarnessIds.ts";
import type { Room } from "./types.ts";

const room = (kinds: string[], prefix: string) =>
	({
		harnesses: kinds.map((k, i) => ({ id: `${prefix}${i}`, kind: k })),
	}) as unknown as Pick<Room, "harnesses">;

describe("contextHarnessIds", () => {
	it("keeps only claude and opencode harnesses, across rooms", () => {
		const ids = contextHarnessIds([
			room(["claude", "shell", "files"], "a"),
			room(["opencode", "copilot"], "b"),
		]);
		expect(ids).toEqual(["a0", "b0"]);
	});

	it("is empty when nothing reports a size", () => {
		expect(contextHarnessIds([room(["shell"], "a")])).toEqual([]);
	});
});
