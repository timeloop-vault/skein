// #418: is the repo at a room's path still the repo the room was made for?
// Pure decision logic, mirroring the Rust side; useRoomsStore.ts does the IO.

import type { RepoIdentity } from "./types.ts";

export type IdentityCheck = "same" | "mismatch" | "unknown";

/** Wire shape of the `git_repo_identity` command. */
export interface RepoIdentityDto {
	rootCommits: string[];
	originUrl?: string | null;
}

export function identityFromDto(dto: RepoIdentityDto): RepoIdentity {
	return dto.originUrl
		? { rootCommits: dto.rootCommits, originUrl: dto.originUrl }
		: { rootCommits: dto.rootCommits };
}

/** Unknown never blocks: only positive evidence of a different repo is a mismatch. */
export function compareIdentity(
	stored: RepoIdentity | undefined,
	folder: { exists: boolean; identity: RepoIdentity | null },
): IdentityCheck {
	if (!stored || stored.rootCommits.length === 0) return "unknown";
	if (!folder.exists) return "unknown";
	if (folder.identity === null) return "mismatch";
	if (folder.identity.rootCommits.length === 0) return "unknown";
	const have = new Set(stored.rootCommits);
	return folder.identity.rootCommits.some((c) => have.has(c)) ? "same" : "mismatch";
}

/** Whether an identity is worth storing (a repo with at least one root commit). */
export function isStorable(identity: RepoIdentity | null): identity is RepoIdentity {
	return identity !== null && identity.rootCommits.length > 0;
}

/** The repoRoot to store after a re-derive: only a verified-same repo may
 *  change it; a room with none keeps the plain fill. */
export function nextRepoRoot(
	current: string | undefined,
	derived: string | undefined,
	check: IdentityCheck,
): string | undefined {
	if (!derived) return current;
	if (!current) return derived;
	return check === "same" ? derived : current;
}
