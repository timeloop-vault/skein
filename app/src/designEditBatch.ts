// designEditBatch — the pure reducer behind "Propose" (#436). Edits to one
// element before it is proposed make ONE thread, so the iframe's stream of
// edit-change beacons is folded here: one entry per style property, one
// offset, one text. The FIRST `from` is kept (what the source says now),
// the latest `to`/`token` wins, and a change that lands back on its
// original value drops out rather than proposing a no-op.

import { type Change, PROPERTY_ALLOWLIST, type Proposal } from "./designProposal.ts";

type StyleChange = Extract<Change, { kind: "style" }>;
type OffsetChange = Extract<Change, { kind: "offset" }>;
type TextChange = Extract<Change, { kind: "text" }>;

export interface Batch {
	style: Readonly<Record<string, StyleChange>>;
	offset?: OffsetChange;
	text?: TextChange;
}

export const emptyBatch = (): Batch => ({ style: {} });

export const applyChange = (batch: Batch, change: Change): Batch => {
	switch (change.kind) {
		case "style": {
			const { [change.property]: prior, ...rest } = batch.style;
			const from = prior ? prior.from : change.from;
			if (change.to === from) return { ...batch, style: rest };
			const next: StyleChange = { kind: "style", property: change.property, from, to: change.to };
			if (change.token !== undefined) next.token = change.token;
			return { ...batch, style: { ...rest, [change.property]: next } };
		}
		case "offset": {
			const { offset: _drop, ...rest } = batch;
			// The iframe reports the cumulative offset, so the latest replaces.
			return change.dx === 0 && change.dy === 0 ? rest : { ...rest, offset: change };
		}
		case "text": {
			const { text: prior, ...rest } = batch;
			const from = prior ? prior.from : change.from;
			return change.to === from ? rest : { ...rest, text: { kind: "text", from, to: change.to } };
		}
	}
};

export const batchSize = (batch: Batch): number =>
	Object.keys(batch.style).length + (batch.offset ? 1 : 0) + (batch.text ? 1 : 0);

/** Style in allowlist order, then offset, then text; null when empty. */
export const batchToProposal = (batch: Batch): Proposal | null => {
	const changes: Change[] = [];
	for (const p of PROPERTY_ALLOWLIST) {
		const c = batch.style[p];
		if (c) changes.push(c);
	}
	if (batch.offset) changes.push(batch.offset);
	if (batch.text) changes.push(batch.text);
	return changes.length > 0 ? { changes } : null;
};
