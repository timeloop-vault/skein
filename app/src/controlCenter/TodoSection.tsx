// The Control Center's second section (#335): the manual todo list. Pure
// renderer over `VisibleTodo[]`; also hosted by the #493 pop-out, so every
// change goes out through `actions` rather than touching state here.

import { useState } from "react";
import { modLabel } from "../shortcuts.ts";
import type { VisibleTodo } from "../todos/model.ts";
import type { TodoActions } from "../todos/useTodos.ts";
import "./TodoSection.css";

interface TodoRowProps {
	item: VisibleTodo;
	actions: TodoActions;
	onFocus: (roomId: string, harnessId?: string) => void;
}

function TodoRow({ item, actions, onFocus }: TodoRowProps) {
	const { scope, todo, roomName, harnessGone } = item;
	const [editing, setEditing] = useState(false);
	const [draft, setDraft] = useState("");
	const done = todo.doneMs !== undefined;
	const focusable = roomName !== null && todo.roomId !== undefined;

	const startEdit = () => {
		setDraft(todo.note ?? "");
		setEditing(true);
	};
	const save = () => {
		actions.setNote(scope, todo.id, draft.trim(), todo.roomId);
		setEditing(false);
	};

	return (
		<div className="cc-todo" data-done={done}>
			<input
				type="checkbox"
				className="cc-todo-check"
				checked={done}
				aria-label={done ? "Mark as not done" : "Mark as done"}
				onChange={(e) => actions.setDone(scope, todo.id, e.target.checked, todo.roomId)}
			/>
			<div className="cc-todo-body">
				<div className="cc-todo-line">
					<button
						type="button"
						className="cc-todo-title"
						disabled={!focusable}
						title={focusable ? "Go to this room" : "The room is closed"}
						onClick={() => {
							if (!focusable || todo.roomId === undefined) return;
							if (todo.harnessId !== undefined && !harnessGone)
								onFocus(todo.roomId, todo.harnessId);
							else onFocus(todo.roomId);
						}}
					>
						{todo.title}
					</button>
					<span className="cc-chip cc-todo-scope" data-scope={scope}>
						{scope === "global" ? "global" : (roomName ?? "room")}
					</span>
					{harnessGone ? <span className="cc-sub cc-todo-hint">harness closed</span> : null}
					<span className="cc-todo-actions">
						<button type="button" className="cc-todo-btn" onClick={startEdit}>
							note
						</button>
						<button
							type="button"
							className="cc-todo-btn"
							aria-label={`Remove todo ${todo.title}`}
							onClick={() => actions.remove(scope, todo.id, todo.roomId)}
						>
							remove
						</button>
					</span>
				</div>
				{editing ? (
					<input
						className="cc-todo-input"
						value={draft}
						// biome-ignore lint/a11y/noAutofocus: opened by an explicit click on "note"
						autoFocus
						placeholder="Note"
						onChange={(e) => setDraft(e.target.value)}
						onBlur={() => setEditing(false)}
						onKeyDown={(e) => {
							if (e.key === "Enter") save();
							else if (e.key === "Escape") setEditing(false);
						}}
					/>
				) : todo.note ? (
					<div className="cc-todo-note">{todo.note}</div>
				) : null}
			</div>
		</div>
	);
}

export function TodoSection({
	todos,
	actions,
	onFocus,
}: {
	todos: readonly VisibleTodo[];
	actions: TodoActions;
	onFocus: (roomId: string, harnessId?: string) => void;
}) {
	return (
		<section className="cc-todos" aria-label="Todos">
			<header className="cc-group">
				<span className="cc-group-label">Todos</span>
				{todos.length > 0 ? <span className="cc-group-count">{todos.length}</span> : null}
			</header>
			{todos.length === 0 ? (
				<div className="cc-todo-empty">
					Nothing to come back to. {modLabel}+T adds the current room or harness, {modLabel}+Shift+T
					a global todo.
				</div>
			) : (
				todos.map((t) => (
					<TodoRow key={`${t.scope}:${t.todo.id}`} item={t} actions={actions} onFocus={onFocus} />
				))
			)}
		</section>
	);
}
