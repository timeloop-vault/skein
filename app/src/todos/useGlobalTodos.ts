import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { mergeLoaded, type Todo } from "./model";

/**
 * The global manual todo list (#335), persisted in sqlite. Loads once on
 * mount; every change after a successful load is mirrored back wholesale.
 * Same guard as `useRoomsHydrate` (#167): a failed load leaves `loaded`
 * false, so the initial [] is never written over the stored list.
 */
export function useGlobalTodos() {
	const [todos, setTodos] = useState<Todo[]>([]);
	const [loaded, setLoaded] = useState(false);

	useEffect(() => {
		let cancelled = false;
		invoke<Todo[]>("db_load_global_todos")
			.then((rows) => {
				if (cancelled) return;
				// Keep a todo added before the load resolved; `loaded` flipping
				// triggers the save effect, so the merged list is persisted.
				setTodos((cur) => mergeLoaded(rows, cur));
				setLoaded(true);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] db_load_global_todos failed:", msg);
			});
		return () => {
			cancelled = true;
		};
	}, []);

	useEffect(() => {
		if (!loaded) return;
		void invoke("db_save_global_todos", { todos }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.error("[skein] db_save_global_todos failed:", msg);
		});
	}, [todos, loaded]);

	return { todos, setTodos, loaded };
}
