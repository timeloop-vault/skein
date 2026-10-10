import { describe, expect, it } from "vitest";
import {
	buildRemoteSpec,
	remoteArgv,
	remoteSessionName,
	shQuote,
	validateRemoteHost,
} from "./remoteCmd.ts";

/** Minimal POSIX unquoter: '...' segments and \x outside quotes only. */
const unquote = (s: string): string => {
	let out = "";
	let i = 0;
	while (i < s.length) {
		const c = s.charAt(i);
		if (c === "'") {
			const end = s.indexOf("'", i + 1);
			if (end < 0) throw new Error("unterminated quote");
			out += s.slice(i + 1, end);
			i = end + 1;
		} else if (c === "\\") {
			out += s.charAt(i + 1);
			i += 2;
		} else {
			out += c;
			i += 1;
		}
	}
	return out;
};

describe("shQuote", () => {
	it("single-quotes", () => {
		expect(shQuote("abc def")).toBe("'abc def'");
	});
	it("escapes an embedded single quote", () => {
		expect(shQuote("a'b")).toBe("'a'\\''b'");
		expect(shQuote("it's")).toBe("'it'\\''s'");
	});
	it("round-trips through a POSIX unquoter", () => {
		for (const x of ["plain", "a'b", "it's 'q'", "~/x y", "$HOME;rm"]) {
			expect(unquote(shQuote(x))).toBe(x);
		}
	});
});

describe("remoteSessionName", () => {
	it("sanitises everything outside [A-Za-z0-9_-]", () => {
		expect(remoteSessionName("r.1:x", "h 2")).toBe("skein-r_1_x-h_2");
		expect(remoteSessionName("room-1", "h_2")).toBe("skein-room-1-h_2");
	});
});

describe("validateRemoteHost", () => {
	it("accepts user@host, host and aliases", () => {
		expect(validateRemoteHost("user@example-host")).toBeNull();
		expect(validateRemoteHost("devbox")).toBeNull();
		expect(validateRemoteHost("my-alias.lan")).toBeNull();
	});
	it("rejects empty, whitespace, control chars and leading dash", () => {
		expect(validateRemoteHost("")).not.toBeNull();
		expect(validateRemoteHost("   ")).not.toBeNull();
		expect(validateRemoteHost("a b")).not.toBeNull();
		expect(validateRemoteHost("a\u0001b")).not.toBeNull();
		expect(validateRemoteHost("-oProxyCommand=x")).not.toBeNull();
	});
});

describe("remoteArgv", () => {
	it("builds the basic argv", () => {
		expect(remoteArgv({ host: "user@example-host", tool: "claude", session: "skein-r-h" })).toEqual(
			[
				"ssh",
				"-t",
				"--",
				"user@example-host",
				"exec \"$SHELL\" -lc 'tmux new-session -A -s '\\''skein-r-h'\\'' '\\''claude'\\'' \\; set-option -t '\\''skein-r-h'\\'' status off'",
			],
		);
	});
	const innerOf = (cmd: string | undefined): string =>
		unquote((cmd ?? "").slice('exec "$SHELL" -lc '.length));
	it("separates tmux commands with a backslash-semicolon", () => {
		const a = remoteArgv({ host: "devbox", tool: "claude", session: "s" });
		expect(a?.[4]).toContain(" \\; set-option");
		const inner = innerOf(a?.[4]);
		expect(inner.split("\\;").length).toBe(2);
		expect(inner).toContain(" \\; set-option");
	});
	it("adds -c for a dir", () => {
		const a = remoteArgv({ host: "devbox", tool: "claude", session: "s", dir: "/srv/app" });
		expect(a?.[4]).toContain("-c '\\''/srv/app'\\''");
	});
	it("expands a leading ~/ via $HOME and a bare ~", () => {
		const home = remoteArgv({ host: "devbox", tool: "claude", session: "s", dir: "~/proj" });
		expect(innerOf(home?.[4])).toContain(" -c \"$HOME\"/'proj' ");
		const bare = remoteArgv({ host: "devbox", tool: "claude", session: "s", dir: "~" });
		expect(innerOf(bare?.[4])).toContain(' -c "$HOME" ');
	});
	it("quotes a tool with a quote and flags", () => {
		const tool = "~/.local/bin/claude --foo 'x'";
		const a = remoteArgv({ host: "devbox", tool, session: "s" });
		expect(innerOf(a?.[4])).toContain(shQuote(tool));
	});
	it("is null on a bad host or empty tool", () => {
		expect(remoteArgv({ host: "-oX", tool: "claude", session: "s" })).toBeNull();
		expect(remoteArgv({ host: "devbox", tool: "  ", session: "s" })).toBeNull();
	});
});

describe("buildRemoteSpec", () => {
	it("trims input and mints the session from the ids", () => {
		const spec = buildRemoteSpec("s_1", "h_2", {
			host: " user@example-host ",
			tool: " claude ",
			dir: " ",
		});
		expect(spec).toEqual({
			host: "user@example-host",
			tool: "claude",
			session: remoteSessionName("s_1", "h_2"),
		});
	});

	it("keeps a non-empty dir", () => {
		const spec = buildRemoteSpec("s", "h", {
			host: "example-host",
			tool: "opencode",
			dir: "/srv/x",
		});
		expect(spec.dir).toBe("/srv/x");
	});
});
