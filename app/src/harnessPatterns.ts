// Generic prompt detection for harness kinds without a native
// adapter — epic #50 L2b.
//
// Claude (L2c-1) and opencode (L2c-2) signal `waiting` via their own
// authoritative event sources. The remaining kinds — copilot, byoh
// (shell, custom commands) — have no structured signal, just bytes
// flowing through the PTY. This module is the fallback: a small
// curated set of regexes that match commonly-blocking prompts
// (sudo password, [y/n] confirmation, "Press Enter to continue").
//
// Scope is deliberately narrow. We don't try to detect "the user is
// at a shell prompt" — that's not a blocked state, that's idle.
// We only fire on prompts that indicate a *child process is waiting
// on user input*. False positives are worse than false negatives
// here: missing a prompt just means the L2a 8 s idle threshold
// catches it; misclassifying every shell prompt as "waiting" would
// pulse the dot every time you sit at a terminal.
//
// Patterns are anchored at end-of-tail with `$` (multiline). Mid-
// history mentions of "password:" don't trigger — the prompt has to
// be the last thing on the screen.

/// Strip ANSI escape sequences + bare control chars from raw PTY
/// output so the pattern matcher sees plain text. Claude / opencode
/// TUIs are heavy escape-sequence emitters, but those kinds use the
/// L2c adapters and skip this path; this strip handles whatever a
/// shell or non-TUI CLI might emit (color codes, bell, cursor
/// moves, OSC for terminal-title updates).
export function stripAnsi(s: string): string {
	// Control characters in regex are intentional here — that's
	// what ANSI escape sequences are. The biome lint exists to
	// prompt review; the answer is yes, we explicitly need
	// 0x1b (ESC) and 0x07 (BEL) and the 0x00-0x1f range.
	// biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI strip
	const csi = /\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]/g;
	// biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI strip
	const osc = /\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g;
	// biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI strip
	const bareCtrl = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/g;
	// Preserves \n \r \t — newlines matter for end-of-line anchoring
	// in `matchesWaitingPrompt`; tabs survive for prompt formatting.
	return s.replace(csi, "").replace(osc, "").replace(bareCtrl, "");
}

// Curated regex set. Each entry must:
//   - Anchor at end-of-line (`$` with `m` flag).
//   - Avoid matching plain shell prompts (`$`, `%`, `#`, `>` alone).
//   - Be case-insensitive (`i` flag) — most prompts use a consistent
//     case but ssh/sudo localisation varies.
// Adding a new pattern: prefer narrow over broad. A false positive
// here lights the dot up incorrectly across every shell session.
const WAITING_PATTERNS: readonly RegExp[] = [
	// (y/n), (yes/no), [y/N], [Y/n] — apt, npm, custom prompts.
	/\((?:y|yes)\/(?:n|no)\)\s*\??\s*$/im,
	/\((?:n|no)\/(?:y|yes)\)\s*\??\s*$/im,
	/\[(?:y\/N|Y\/n|yes\/no|y\/n)\]\s*\??\s*$/im,
	// "Press Enter to continue", "Press any key" — CLI pause prompts.
	/\bpress\s+(?:enter|any\s+key)\b[^\n]*$/im,
	// "Password:", "[sudo] password for user:" — sudo, ssh, custom auth.
	/^\s*password\s*:\s*$/im,
	/\bpassword\s+for\s+\S+\s*:\s*$/im,
	// "passphrase for /path:" — ssh-add, git over ssh.
	/\bpassphrase\b[^:\n]*:\s*$/im,
	// "Continue? [y/N]" — git, gh, custom CLIs.
	/\bcontinue\b[^?\n]*\?\s*\[[ynYN/]+\]\s*$/im,
];

/// Test the tail of a harness's output buffer for a "waiting-on-
/// user-input" prompt. Only scans the last ~256 chars — the prompt
/// is by definition the last thing on screen, and limiting the
/// scan keeps the matcher cheap (the L2a tick calls this every
/// second for every non-authoritative harness).
///
/// Returns `true` if any curated pattern matches.
export function matchesWaitingPrompt(tail: string): boolean {
	const end = tail.length > 256 ? tail.slice(-256) : tail;
	for (const pattern of WAITING_PATTERNS) {
		if (pattern.test(end)) return true;
	}
	return false;
}
