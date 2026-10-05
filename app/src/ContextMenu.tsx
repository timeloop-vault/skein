// The app's reusable right-click menu. One instance is hosted by whoever
// owns `useContextMenu` (todos/TodoMenuProvider.tsx today); tabs only call
// `open(event, items)`. Closes on Esc, an outside press, blur, scroll,
// resize and after a choice; arrows/Home/End move focus, Enter activates.

import {
	type MouseEvent as ReactMouseEvent,
	useCallback,
	useEffect,
	useLayoutEffect,
	useRef,
	useState,
} from "react";
import { clampMenuPosition, nextMenuIndex, type Point } from "./contextMenuModel.ts";
import "./ContextMenu.css";

export interface ContextMenuItem {
	id: string;
	label: string;
	onSelect: () => void;
	disabled?: boolean;
}

interface OpenMenu {
	at: Point;
	items: readonly ContextMenuItem[];
}

export function useContextMenu() {
	const [menu, setMenu] = useState<OpenMenu | null>(null);
	const open = useCallback((e: ReactMouseEvent, items: readonly ContextMenuItem[]) => {
		e.preventDefault();
		e.stopPropagation();
		setMenu({ at: { x: e.clientX, y: e.clientY }, items });
	}, []);
	const close = useCallback(() => setMenu(null), []);
	return { menu, open, close };
}

export const ContextMenu = ({
	at,
	items,
	onClose,
}: {
	at: Point;
	items: readonly ContextMenuItem[];
	onClose: () => void;
}) => {
	const ref = useRef<HTMLDivElement | null>(null);
	const [pos, setPos] = useState<Point>(at);

	// Remember what had focus before the menu took it; give it back on close
	// (Esc, outside press, a choice: all unmount the menu).
	useLayoutEffect(() => {
		const prev = document.activeElement;
		return () => {
			if (prev instanceof HTMLElement && prev.isConnected) prev.focus();
		};
	}, []);

	// Measure after mount, then clamp inside the viewport and take focus.
	useLayoutEffect(() => {
		const el = ref.current;
		if (!el) return;
		const rect = el.getBoundingClientRect();
		setPos(
			clampMenuPosition(
				at,
				{ width: rect.width, height: rect.height },
				{ width: window.innerWidth, height: window.innerHeight },
			),
		);
		el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
	}, [at]);

	useEffect(() => {
		const onPointerDown = (e: MouseEvent) => {
			if (ref.current && !ref.current.contains(e.target as Node)) onClose();
		};
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				onClose();
			}
		};
		document.addEventListener("mousedown", onPointerDown);
		document.addEventListener("keydown", onKey);
		window.addEventListener("blur", onClose);
		window.addEventListener("resize", onClose);
		window.addEventListener("scroll", onClose, true);
		return () => {
			document.removeEventListener("mousedown", onPointerDown);
			document.removeEventListener("keydown", onKey);
			window.removeEventListener("blur", onClose);
			window.removeEventListener("resize", onClose);
			window.removeEventListener("scroll", onClose, true);
		};
	}, [onClose]);

	const onMenuKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
		const buttons = Array.from(
			ref.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? [],
		);
		const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
		const next = nextMenuIndex(e.key, current, buttons.length);
		if (next === null) return;
		e.preventDefault();
		buttons[next]?.focus();
	};

	return (
		<div
			ref={ref}
			className="sk-context-menu"
			role="menu"
			style={{ left: pos.x, top: pos.y }}
			onKeyDown={onMenuKeyDown}
			onContextMenu={(e) => e.preventDefault()}
		>
			{items.map((item) => (
				<button
					key={item.id}
					type="button"
					role="menuitem"
					className="sk-context-menu-item"
					disabled={item.disabled === true}
					onClick={() => {
						onClose();
						item.onSelect();
					}}
				>
					{item.label}
				</button>
			))}
		</div>
	);
};
