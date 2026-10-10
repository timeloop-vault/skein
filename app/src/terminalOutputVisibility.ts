// Ties a terminal's output gate (#592) to whether its host is on screen.
//
// "Hidden" is the same test observeResize uses to skip fitting: a 0×0
// content box (display:none on any ancestor gives that). The observer is
// created here, before any other one on the host, because callbacks run in
// creation order and the reveal flush must land before the refit.

import { createOutputGate, type OutputGate } from "./terminalOutputGate.ts";

interface HostBox {
	clientWidth: number;
	clientHeight: number;
}

export function isHostHidden(host: HostBox): boolean {
	return host.clientWidth === 0 || host.clientHeight === 0;
}

export function attachOutputGate(
	term: { write(data: string, callback?: () => void): void },
	host: HostBox & Element,
	RO: typeof ResizeObserver = ResizeObserver,
): { gate: OutputGate; detach: () => void } {
	const gate = createOutputGate(
		{ write: (data, onParsed) => term.write(data, onParsed) },
		isHostHidden(host),
	);
	const observer = new RO(() => gate.setHidden(isHostHidden(host)));
	observer.observe(host);
	return {
		gate,
		detach: () => {
			observer.disconnect();
			gate.dispose();
		},
	};
}
