// Card stack — the resizable, collapsible cards. Shared chrome;
// bespoke bodies passed in by the parent. Drag-resize redistributes
// flex weight between adjacent cards (min 8% each, handover §8);
// collapse forces a card to its head height. The parent persists the
// layout per room.
//
// The arity is dynamic rather than a fixed tuple. It was [Diff, Plan,
// Activity] until #212 moved the diff into the review pane, and paying
// for that with a tuple rewrite — plus a migration for every persisted
// three-element layout — taught the lesson once. `normalizeLayout`
// reconciles whatever localStorage holds with however many cards the
// caller passes today.

import { Fragment, type ReactNode, useRef } from "react";

/// Per-room card layout: flex weights + collapsed flags, positional
/// against the `cards` array. Persisted to localStorage keyed by room.
export interface CardLayout {
	weights: number[];
	collapsed: boolean[];
}

/// Minimum flex weight per card while dragging — 8% of the total
/// (handover §8) so a card can't be dragged to nothing.
const MIN_WEIGHT_FRACTION = 0.08;

/// Fit a stored layout to `count` cards: pad with even weights, drop
/// the tail, and repair a blob that is not the right shape at all.
///
/// A stored layout is user data from an older version, so it is treated
/// like one (#167's field policy in spirit): never trusted for its
/// length, never thrown away for being the wrong one.
export function normalizeLayout(layout: CardLayout | undefined, count: number): CardLayout {
	const weights = Array.from({ length: count }, (_, i) => {
		const w = layout?.weights?.[i];
		return typeof w === "number" && Number.isFinite(w) && w > 0 ? w : 1;
	});
	const collapsed = Array.from({ length: count }, (_, i) => layout?.collapsed?.[i] === true);
	return { weights, collapsed };
}

export const defaultLayout = (count: number): CardLayout => ({
	weights: Array.from({ length: count }, () => 1),
	collapsed: Array.from({ length: count }, () => false),
});

export interface CardDef {
	label: string;
	meta: ReactNode;
	body: ReactNode;
}

interface CardStackProps {
	layout: CardLayout;
	onLayoutChange: (next: CardLayout) => void;
	onToggleCollapse: (i: number) => void;
	cards: CardDef[];
}

export const CardStack = ({ layout, onLayoutChange, onToggleCollapse, cards }: CardStackProps) => {
	const stackRef = useRef<HTMLDivElement>(null);
	const fitted = normalizeLayout(layout, cards.length);

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
		const startWeights = [...fitted.weights];
		const sum = startWeights.reduce((a, b) => a + b, 0);
		const minW = sum * MIN_WEIGHT_FRACTION;
		// The two cards either side of this divider.
		const wA = startWeights[i] ?? 0;
		const wB = startWeights[i + 1] ?? 0;

		let latest = fitted;
		const onMove = (ev: PointerEvent) => {
			const deltaPx = ev.clientY - startY;
			let deltaW = (deltaPx / height) * sum;
			// Clamp so neither adjacent card drops below the minimum.
			deltaW = Math.max(minW - wA, Math.min(deltaW, wB - minW));
			const weights = [...startWeights];
			weights[i] = wA + deltaW;
			weights[i + 1] = wB - deltaW;
			latest = { ...fitted, weights };
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
						className={`lc-card ${fitted.collapsed[i] ? "collapsed" : ""}`}
						style={{ flex: fitted.weights[i] }}
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
