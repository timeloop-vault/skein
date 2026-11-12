// Default agent per harness kind (#248), daemon-owned since #730.
//
// The value used to live in localStorage ("skein:defaultAgents"). The
// daemon now owns it in settings.json; the legacy key is read once per
// boot and offered to `default_agents_import`, which only ever takes effect
// the first time. The legacy key is deliberately never written or removed,
// so an older build after a downgrade still finds it.

import type { DefaultAgents } from "./prefs.ts";
import type { HarnessKind } from "./types.ts";

export const LEGACY_DEFAULT_AGENTS_KEY = "skein:defaultAgents";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type ReadStorage = (key: string) => string | null;

export interface DefaultAgentsDeps {
	invoke: Invoke;
	readStorage: ReadStorage;
}

/** The legacy localStorage map; absent, unparseable or wrongly shaped -> `{}`. */
export const readLegacyDefaultAgents = (readStorage: ReadStorage): DefaultAgents => {
	try {
		const raw = readStorage(LEGACY_DEFAULT_AGENTS_KEY);
		if (raw === null) return {};
		const parsed: unknown = JSON.parse(raw);
		if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
		return parsed as DefaultAgents;
	} catch {
		return {};
	}
};

/** Offer the legacy value to the daemon and return the daemon's map. */
export const loadDefaultAgents = async ({
	invoke,
	readStorage,
}: DefaultAgentsDeps): Promise<DefaultAgents> => {
	const result = await invoke<{ imported: boolean; defaultAgents: DefaultAgents }>(
		"default_agents_import",
		{ defaultAgents: readLegacyDefaultAgents(readStorage) },
	);
	return result.defaultAgents;
};

/** Set (or, with undefined, clear) one kind's default; returns the new map. */
export const setDefaultAgent = (
	{ invoke }: Pick<DefaultAgentsDeps, "invoke">,
	kind: HarnessKind,
	agent: string | undefined,
): Promise<DefaultAgents> =>
	invoke<DefaultAgents>("default_agent_set", { kind, agent: agent ?? null });
