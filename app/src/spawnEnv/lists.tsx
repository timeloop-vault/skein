import type { EnvVar } from "../types.ts";

/** A list of freeform strings with add / remove / reorder. */
export const StringList = ({
	items,
	placeholder,
	addLabel,
	onChange,
}: {
	items: string[];
	placeholder: string;
	addLabel: string;
	onChange: (next: string[]) => void;
}) => {
	const replace = (i: number, value: string) =>
		onChange(items.map((v, j) => (j === i ? value : v)));
	const move = (i: number, delta: number) => {
		const j = i + delta;
		if (j < 0 || j >= items.length) return;
		const next = [...items];
		const [row] = next.splice(i, 1);
		if (row === undefined) return;
		next.splice(j, 0, row);
		onChange(next);
	};
	return (
		<div className="sk-list">
			{items.map((item, i) => (
				// Index keys are correct here: rows are positional, the
				// list is short, and the value itself is user-editable
				// (so it is not a stable identity).
				<div className="sk-list-row" key={i}>
					<input
						className="sk-input"
						value={item}
						placeholder={placeholder}
						spellCheck={false}
						onChange={(e) => replace(i, e.target.value)}
					/>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Move up"
						disabled={i === 0}
						onClick={() => move(i, -1)}
					>
						↑
					</button>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Move down"
						disabled={i === items.length - 1}
						onClick={() => move(i, 1)}
					>
						↓
					</button>
					<button
						type="button"
						className="sk-btn sk-list-btn"
						title="Remove"
						onClick={() => onChange(items.filter((_, j) => j !== i))}
					>
						✕
					</button>
				</div>
			))}
			<button type="button" className="sk-btn sk-list-add" onClick={() => onChange([...items, ""])}>
				{addLabel}
			</button>
		</div>
	);
};

export const EnvVarList = ({
	items,
	onChange,
}: {
	items: EnvVar[];
	onChange: (next: EnvVar[]) => void;
}) => (
	<div className="sk-list">
		{items.map((item, i) => (
			<div className="sk-list-row" key={i}>
				<input
					className="sk-input sk-env-key"
					value={item.key}
					placeholder="NAME"
					spellCheck={false}
					onChange={(e) =>
						onChange(items.map((v, j) => (j === i ? { ...v, key: e.target.value } : v)))
					}
				/>
				<input
					className="sk-input"
					value={item.value}
					placeholder="value"
					spellCheck={false}
					onChange={(e) =>
						onChange(items.map((v, j) => (j === i ? { ...v, value: e.target.value } : v)))
					}
				/>
				<button
					type="button"
					className="sk-btn sk-list-btn"
					title="Remove"
					onClick={() => onChange(items.filter((_, j) => j !== i))}
				>
					✕
				</button>
			</div>
		))}
		<button
			type="button"
			className="sk-btn sk-list-add"
			onClick={() => onChange([...items, { key: "", value: "" }])}
		>
			Add variable
		</button>
	</div>
);
