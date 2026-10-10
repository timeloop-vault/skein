import { describe, expect, it } from "vitest";
import { attachOutputGate } from "./terminalOutputVisibility.ts";

function setup(width: number, height: number) {
	const host = { clientWidth: width, clientHeight: height } as unknown as HTMLDivElement;
	const written: string[] = [];
	const term = {
		write(data: string, cb?: () => void) {
			written.push(data);
			cb?.();
		},
	};
	let fire: () => void = () => {};
	let disconnected = false;
	class FakeRO {
		constructor(cb: () => void) {
			fire = cb;
		}
		observe() {}
		disconnect() {
			disconnected = true;
		}
	}
	const attached = attachOutputGate(term, host, FakeRO as unknown as typeof ResizeObserver);
	return {
		...attached,
		host: host as { clientWidth: number; clientHeight: number },
		written,
		fire: () => fire(),
		wasDisconnected: () => disconnected,
	};
}

describe("attachOutputGate", () => {
	it("passes writes straight through while the host is visible", () => {
		const s = setup(800, 600);
		s.gate.write("a");
		expect(s.written).toEqual(["a"]);
	});

	it("buffers while the host is 0x0 and flushes on reveal", () => {
		const s = setup(0, 0);
		s.gate.write("a");
		expect(s.written).toEqual([]);
		s.host.clientWidth = 800;
		s.host.clientHeight = 600;
		s.fire();
		expect(s.written.join("")).toBe("a");
	});

	it("buffers again when the host is hidden later", () => {
		const s = setup(800, 600);
		s.host.clientWidth = 0;
		s.fire();
		s.gate.write("b");
		expect(s.written).toEqual([]);
		s.detach();
		expect(s.wasDisconnected()).toBe(true);
	});
});
