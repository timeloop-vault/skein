import type { CaptureMode, DropReason } from "../types.ts";

export const CAPTURE_OPTIONS: { value: CaptureMode; label: string; desc: string }[] = [
	{
		value: "login-interactive",
		label: "Login + interactive shell",
		desc: "Sources your whole startup chain (.zshenv, .zprofile, .zshrc). Closest to what a new terminal window gives you — interactive-only files are where version managers and package managers install themselves.",
	},
	{
		value: "login",
		label: "Login files only",
		desc: "Skips .zshrc and friends. Faster, and avoids prompt frameworks and completion loading. Use this if your interactive config is slow or fragile.",
	},
	{
		value: "none",
		label: "Don't ask a shell",
		desc: "Use the environment Skein itself was launched with, plus your additions below. Honest choice if you always start Skein from a terminal — and the escape hatch if a probe misbehaves.",
	},
];

export const PROBE_TONE: Record<string, "ok" | "warn" | "err" | "muted"> = {
	captured: "ok",
	pending: "muted",
	// A deliberate setting, not a failure — see ProbeFailure::Disabled.
	disabled: "muted",
	not_applicable: "muted",
	unsupported_shell: "warn",
	timeout: "err",
	spawn_failed: "err",
	no_payload: "warn",
};

export const DROP_REASON: Record<DropReason, string> = {
	unresolved: "unset variable",
	not_absolute: "not an absolute path",
	separator: "contains a path separator",
	missing: "directory not found",
	duplicate: "already on PATH",
};

export const SOURCE_LABEL: Record<string, string> = {
	added: "yours",
	shell: "shell",
	inherited: "inherited",
};
