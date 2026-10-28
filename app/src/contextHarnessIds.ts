// Ids of the harnesses that report a context size (#368), for the room and
// group tab marks. Returned as a JSON key too so callers can memoize on a
// primitive.

import { CONTEXT_SIZE_KINDS } from "./contextSize.ts";
import type { Room } from "./types.ts";

export function contextHarnessIds(rooms: readonly Pick<Room, "harnesses">[]): string[] {
	return rooms.flatMap((r) =>
		r.harnesses.filter((h) => CONTEXT_SIZE_KINDS.has(h.kind)).map((h) => h.id),
	);
}
