// DesignEditPanel — the side column while an element is picked for editing
// (#436): its allowlisted style properties, the pending batch, and the note
// that rides on the proposal thread. Values from the page are text only.

import { useState } from "react";
import type { Batch } from "./designEditBatch.ts";
import { type Change, changeLine, type DesignToken } from "./designProposal.ts";
import type { ElementDescriptor } from "./elementAnchor.ts";
import "./design.css";

const GROUPS: readonly { label: string; props: readonly string[] }[] = [
	{ label: "Size", props: ["width", "height"] },
	{
		label: "Spacing",
		props: [
			"margin-top",
			"margin-right",
			"margin-bottom",
			"margin-left",
			"padding-top",
			"padding-right",
			"padding-bottom",
			"padding-left",
			"gap",
		],
	},
	{ label: "Colour", props: ["color", "background-color", "border-color"] },
	{
		label: "Font",
		props: ["font-family", "font-size", "font-weight", "line-height", "letter-spacing"],
	},
	{ label: "Shape", props: ["border-radius", "opacity"] },
];

const TOKEN_LIST = "dp-edit-tokens";

const PropertyInput = ({
	property,
	value,
	onCommit,
}: {
	property: string;
	value: string;
	onCommit: (value: string) => void;
}) => (
	<label className="dp-edit-prop">
		<span>{property}</span>
		<input
			key={value}
			type="text"
			list={TOKEN_LIST}
			defaultValue={value}
			spellCheck={false}
			onKeyDown={(e) => {
				e.stopPropagation();
				if (e.key === "Enter") e.currentTarget.blur();
			}}
			onBlur={(e) => {
				const v = e.currentTarget.value.trim();
				if (v !== value && v !== "") onCommit(v);
				else e.currentTarget.value = value;
			}}
		/>
	</label>
);

const batchChanges = (batch: Batch): Change[] => [
	...Object.values(batch.style),
	...(batch.offset ? [batch.offset] : []),
	...(batch.text ? [batch.text] : []),
];

export const DesignEditPanel = ({
	target,
	computed,
	tokens,
	batch,
	busy,
	error,
	onSetProperty,
	onPropose,
	onDiscard,
}: {
	target: ElementDescriptor;
	computed: Record<string, string>;
	tokens: DesignToken[];
	batch: Batch;
	busy: boolean;
	error: string | null;
	onSetProperty: (property: string, value: string) => void;
	onPropose: (note: string) => void;
	onDiscard: () => void;
}) => {
	const [note, setNote] = useState("");
	const changes = batchChanges(batch);
	const label = `<${target.tag}>${target.text ? ` ${target.text}` : ""}`;
	return (
		<div className="dp-comments dp-edit">
			<div className="dp-comment-el" title={label}>
				{label}
			</div>
			<div className="dp-edit-hint">
				drag to move · arrows nudge 1px (Shift 8px) · handles resize · double-click text to edit
			</div>
			<datalist id={TOKEN_LIST}>
				{tokens.map((t) => (
					<option key={t.name} value={`var(${t.name})`}>
						{t.value}
					</option>
				))}
				{tokens.map((t) => (
					<option key={`raw-${t.name}`} value={t.value} />
				))}
			</datalist>
			{GROUPS.map((g) => (
				<fieldset key={g.label} className="dp-edit-group">
					<legend>{g.label}</legend>
					{g.props.map((p) => (
						<PropertyInput
							key={p}
							property={p}
							value={batch.style[p]?.to ?? computed[p] ?? ""}
							onCommit={(v) => onSetProperty(p, v)}
						/>
					))}
				</fieldset>
			))}
			<div className="dp-edit-batch">
				{changes.length === 0 ? (
					<span className="dp-edit-hint">no changes yet</span>
				) : (
					<ul>
						{changes.map((c) => {
							const line = changeLine(c);
							return <li key={line}>{line}</li>;
						})}
					</ul>
				)}
			</div>
			<textarea
				className="rv-composer-input"
				placeholder="Note for the agent (optional)…"
				rows={3}
				value={note}
				disabled={busy}
				onChange={(e) => setNote(e.target.value)}
				onKeyDown={(e) => e.stopPropagation()}
			/>
			{error !== null && (
				<div className="fp-notice warn" role="alert">
					{error}
				</div>
			)}
			<div className="dp-edit-actions">
				<button
					type="button"
					disabled={busy || changes.length === 0}
					onClick={() => onPropose(note.trim())}
				>
					Propose
				</button>
				<button type="button" disabled={busy} onClick={onDiscard}>
					Discard
				</button>
			</div>
		</div>
	);
};
