// Card stack — the three resizable, collapsible cards. Shared chrome;
// bespoke bodies passed in by the parent. Drag-resize redistributes
// flex weight between adjacent cards (min 8% each, handover §8);
// collapse forces a card to its head height. The parent persists the
// layout per room.

import { Fragment, type ReactNode, useRef } from "react";

/// Per-room card layout: flex weights + collapsed flags, indices
/// [diff, plan, activity]. Persisted to localStorage keyed by room.
export interface CardLayout {
	weights: [number, number, number];
	collapsed: [boolean, boolean, boolean];
}

export const DEFAULT_LAYOUT: CardLayout = {
	weights: [1, 1, 1],
	collapsed: [false, false, false],
};

/// Minimum flex weight per card while dragging — 8% of the total
/// (handover §8) so a card can't be dragged to nothing.
const MIN_WEIGHT_FRACTION = 0.08;

export interface CardDef {
	label: string;
	meta: ReactNode;
	body: ReactNode;
}

interface CardStackProps {
	layout: CardLayout;
	onLayoutChange: (next: CardLayout) => void;
	onToggleCollapse: (i: number) => void;
	cards: [CardDef, CardDef, CardDef];
}

export const CardStack = ({ layout, onLayoutChange, onToggleCollapse, cards }: CardStackProps) => {
	const stackRef = useRef<HTMLDivElement>(null);

	// Drag a divider between card `i` and `i+1`: move flex weight from
	// one to the other, proportional to the pointer's vertical travel
	// over the stack height. Persisted on release via onLayoutChange.
	const onDividerDown = (i: number) => (e: React.PointerEvent) => {
		e.preventDefault();
		const stack = stackRef.current;
		if (!stack) return;
		const height = stack.getBoundingClientRect().height;
		if (height <= 0) return;
		const startY = e.clientY;
		const startWeights = [...layout.weights] as [number, number, number];
		const sum = startWeights[0] + startWeights[1] + startWeights[2];
		const minW = sum * MIN_WEIGHT_FRACTION;
		// The two cards either side of this divider. Captured up front
		// so the move handler does no tuple-by-variable indexing.
		const wA = startWeights[i] ?? 0;
		const wB = startWeights[i + 1] ?? 0;

		let latest = layout;
		const onMove = (ev: PointerEvent) => {
			const deltaPx = ev.clientY - startY;
			let deltaW = (deltaPx / height) * sum;
			// Clamp so neither adjacent card drops below the minimum.
			deltaW = Math.max(minW - wA, Math.min(deltaW, wB - minW));
			const weights = [...startWeights] as [number, number, number];
			weights[i] = wA + deltaW;
			weights[i + 1] = wB - deltaW;
			latest = { ...layout, weights };
			onLayoutChange(latest);
		};
		const onUp = () => {
			window.removeEventListener("pointermove", onMove);
			window.removeEventListener("pointerup", onUp);
			onLayoutChange(latest);
		};
		window.addEventListener("pointermove", onMove);
		window.addEventListener("pointerup", onUp);
	};

	return (
		<div className="lc-stack" ref={stackRef}>
			{cards.map((card, i) => (
				<Fragment key={card.label}>
					<div
						className={`lc-card ${layout.collapsed[i] ? "collapsed" : ""}`}
						style={{ flex: layout.weights[i] }}
					>
						<div className="lc-card-head" onClick={() => onToggleCollapse(i)}>
							<span className="chev">▾</span>
							<span className="label">{card.label}</span>
							<span className="meta">{card.meta}</span>
						</div>
						<div className="lc-card-body">{card.body}</div>
					</div>
					{/* Pointer-drag resize affordance. Keyboard resize is
					    out of scope for v1 (handover §8), so no ARIA role —
					    it's a mouse-only divider, not a focusable widget. */}
					{i < cards.length - 1 && <div className="lc-divider" onPointerDown={onDividerDown(i)} />}
				</Fragment>
			))}
		</div>
	);
};
