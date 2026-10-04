// Frames the design preview iframe at a device size (#528). With no size
// the children render untouched (None behaves exactly as before). With one,
// the iframe keeps its true CSS size and is scaled down with a transform to
// fit the pane (never up); the outer box takes the scaled size so the stage
// can centre it. Size changes reflow the page (no reload): picker.js's
// resize listener re-locates the pins.

import { type ReactNode, useLayoutEffect, useRef, useState } from "react";
import { type DesignDevice, fitScale, frameSize } from "./designDevice.ts";
import "./designDevice.css";

interface Props {
	device: DesignDevice | undefined;
	children: ReactNode;
}

export const DesignDeviceStage = ({ device, children }: Props) => {
	const size = frameSize(device);
	const stageRef = useRef<HTMLDivElement | null>(null);
	const [avail, setAvail] = useState({ width: 0, height: 0 });
	const sized = size !== null;

	useLayoutEffect(() => {
		const el = stageRef.current;
		if (!sized || !el) return;
		const measure = () => setAvail({ width: el.clientWidth, height: el.clientHeight });
		measure();
		const ro = new ResizeObserver(measure);
		ro.observe(el);
		return () => ro.disconnect();
	}, [sized]);

	// One element tree in both cases, so toggling None never remounts the
	// iframe; None is styled by CSS to fill exactly as before.
	const s = size === null ? 1 : fitScale(size, avail);
	const style =
		size === null
			? undefined
			: ({
					"--dd-w": `${size.width}px`,
					"--dd-h": `${size.height}px`,
					"--dd-s": String(s),
					width: size.width * s,
					height: size.height * s,
				} as React.CSSProperties);
	return (
		<div className={`dd-stage${sized ? "" : " dd-fill"}`} ref={stageRef}>
			<div className="dd-box" style={style} data-measured={avail.width > 0} data-scale={s}>
				{children}
			</div>
		</div>
	);
};
