import { describe, expect, it } from "vitest";
import { buildRemoteSpec, remoteSessionName, validateRemoteHost } from "./remoteCmd.ts";

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
