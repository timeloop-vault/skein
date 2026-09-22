// Room naming (#241): the default name a new room gets, and the rule
// for committing an inline rename edit. Pure module — no React.

/// The trailing path component of `cwd`, used as a new room's default
/// display name — `D:\code\skein` → `skein`, `/a/skein-wt/skein-241/`
/// → `skein-241`. Mirrors the folder-name derivation `createRoom` used
/// inline before #241 (trailing `/`/`\` stripped, split on either),
/// falling back to `cwd` itself when there's no trailing component to
/// take (e.g. a bare drive root).
export function defaultRoomName(cwd: string): string {
	return (
		cwd
			.replace(/[\\/]+$/, "")
			.split(/[\\/]/)
			.pop() || cwd
	);
}

/// What an inline rename commits: the trimmed `input`, or `previous`
/// unchanged if trimming leaves nothing. Used on Enter/blur so an
/// empty or whitespace-only edit can't blank out a room's name.
export function commitRoomName(previous: string, input: string): string {
	const trimmed = input.trim();
	return trimmed.length > 0 ? trimmed : previous;
}
