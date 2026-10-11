// The "Env profile" section of the harness Actions menu (#615): the
// harness's own override, or follow the global profile. Presentation only —
// the item model is envProfileMenu.ts.

import { envProfileMenuItems } from "./envProfileMenu.ts";
import type { Harness } from "./types.ts";

export const EnvProfileMenuSection = ({
	harness,
	active,
	names,
	onChoose,
}: {
	harness: Harness;
	active: string | null;
	names: string[];
	/** undefined = follow global (removes the override). */
	onChoose: (name: string | undefined) => void;
}) => (
	<>
		<div className="sk-harness-actions-divider">Env profile</div>
		{envProfileMenuItems(harness.envProfile, active, names).map((item) => (
			<button
				key={item.id}
				type="button"
				className="sk-harness-actions-item"
				role="menuitemradio"
				aria-checked={item.checked}
				title={
					item.missing
						? "This profile no longer exists; the global profile applies. Pick another to replace it."
						: undefined
				}
				onClick={() => onChoose(item.value)}
			>
				{item.checked ? "✓ " : ""}
				{item.label}
			</button>
		))}
	</>
);
