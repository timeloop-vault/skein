// Command palette — Ctrl+K to switch to anything or run any common
// command. Rooms, harnesses, and built-in actions live in one
// flat list with substring filtering.

import { useEffect, useMemo, useRef, useState } from "react";
import { useFocusRestore } from "./useFocusRestore.ts";

export interface PaletteItem {
	id: string;
	label: string;
	// Optional dim text shown right-aligned (e.g. group name, shortcut hint).
	hint?: string;
	invoke: () => void;
}

interface CommandPaletteProps {
	items: PaletteItem[];
	onClose: () => void;
}

export const CommandPalette = ({ items, onClose }: CommandPaletteProps) => {
	useFocusRestore();
	const [query, setQuery] = useState("");
	const [selectedIdx, setSelectedIdx] = useState(0);
	const inputRef = useRef<HTMLInputElement>(null);

	useEffect(() => {
		inputRef.current?.focus();
	}, []);

	const filtered = useMemo(() => {
		const q = query.trim().toLowerCase();
		if (!q) return items;
		return items.filter((it) => it.label.toLowerCase().includes(q));
	}, [items, query]);

	// Reset selection when filter changes — clamping to a valid row.
	useEffect(() => {
		setSelectedIdx((idx) => {
			if (filtered.length === 0) return 0;
			return Math.min(idx, filtered.length - 1);
		});
	}, [filtered.length]);

	const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
		if (e.key === "Escape") {
			e.preventDefault();
			onClose();
			return;
		}
		if (filtered.length === 0) return;
		if (e.key === "ArrowDown") {
			e.preventDefault();
			setSelectedIdx((idx) => (idx + 1) % filtered.length);
		} else if (e.key === "ArrowUp") {
			e.preventDefault();
			setSelectedIdx((idx) => (idx - 1 + filtered.length) % filtered.length);
		} else if (e.key === "Enter") {
			e.preventDefault();
			const item = filtered[selectedIdx];
			if (item) {
				item.invoke();
				onClose();
			}
		}
	};

	return (
		<div className="sk-modal-bg" onClick={onClose}>
			<div className="sk-palette" onClick={(e) => e.stopPropagation()}>
				<input
					ref={inputRef}
					className="sk-palette-input"
					placeholder="Switch to a room, run a command…"
					value={query}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={onKeyDown}
				/>
				<div className="sk-palette-list">
					{filtered.length === 0 && <div className="sk-palette-empty">No matches.</div>}
					{filtered.map((item, i) => (
						<div
							key={item.id}
							className={`sk-palette-row ${i === selectedIdx ? "selected" : ""}`}
							onMouseEnter={() => setSelectedIdx(i)}
							onClick={() => {
								item.invoke();
								onClose();
							}}
						>
							<span className="label">{item.label}</span>
							{item.hint && <span className="hint">{item.hint}</span>}
						</div>
					))}
				</div>
			</div>
		</div>
	);
};
