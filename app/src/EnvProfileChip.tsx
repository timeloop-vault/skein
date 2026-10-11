// Status-bar chip for the global env profile (#615). Hidden until at least
// one profile exists. The menu is the app's generic ContextMenu, opened from
// a left click.

import { ContextMenu, type ContextMenuItem, useContextMenu } from "./ContextMenu.tsx";
import { useEnvProfiles } from "./envProfileStore.ts";
import { effectiveProfile, useSpawnedProfile } from "./spawnedEnvProfile.ts";
import { staleTitle } from "./staleProfiles.ts";
import type { Harness } from "./types.ts";
import "./EnvProfileChip.css";

const TIP = "Environment profile for new harnesses — applies on next start/restart";

export const EnvProfileChip = ({
	activeHarness,
	onSwitch,
}: {
	activeHarness: Harness;
	onSwitch: (name: string | null) => void;
}) => {
	const { names, active, loaded } = useEnvProfiles();
	const { menu, open, close } = useContextMenu();
	const spawned = useSpawnedProfile(activeHarness.id);
	if (names.length === 0) return null;

	const stale = loaded
		? staleTitle(spawned, effectiveProfile(activeHarness.envProfile, active, names))
		: null;
	const items: ContextMenuItem[] = [
		{ id: "none", label: `${active === null ? "✓ " : "   "}None`, onSelect: () => onSwitch(null) },
		...names.map((n) => ({
			id: `p:${n}`,
			label: `${active === n ? "✓ " : "   "}${n}`,
			onSelect: () => onSwitch(n),
		})),
	];
	return (
		<>
			<button
				type="button"
				className={`seg sk-envchip${stale ? " stale" : ""}`}
				title={stale ?? TIP}
				onClick={(e) => open(e, items)}
			>
				env: {active ?? "none"}
				{stale && <span className="sk-envchip-stale">↻</span>}
			</button>
			{menu && <ContextMenu at={menu.at} items={menu.items} onClose={close} />}
		</>
	);
};
