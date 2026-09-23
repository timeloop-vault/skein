// Standalone window/app-level effects, extracted out of App.tsx (#19)
// — pure move, no behaviour change. Owns the boot-time default-shell /
// default-cwd probe, the quit-confirmation wiring (#185's onCloseRequested
// + the macOS Cmd+Q skein://quit-requested route), the Esc-closes-the-
// harness-picker listener (#189), the skein://open-settings listener
// (Preferences… menu item), the #132 shared status-popover attach, and
// the #120 stray-file-drop swallow. None of these six interact with
// each other or with anything else App.tsx still declares between
// their old positions — each only closes over the setters/values
// passed in here — so they moved into one hook call, at the point the
// earliest of them (the boot-time default probe) used to sit.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { confirm } from "@tauri-apps/plugin-dialog";
import type { Dispatch, SetStateAction } from "react";
import { useEffect, useRef } from "react";
import { filesRegistry } from "./filesRegistry.ts";
import { attachStatusPopover } from "./statusPopover.ts";

export function useAppWindowEffects(
	showPicker: string | null,
	setShowPicker: Dispatch<SetStateAction<string | null>>,
	setShowSettings: Dispatch<SetStateAction<boolean>>,
	setDefaultShell: Dispatch<SetStateAction<string[]>>,
	setDefaultCwd: Dispatch<SetStateAction<string>>,
) {
	// Phase 1: pull platform defaults once at boot. New harnesses spawn
	// into these until Phase 4 wires real worktrees / per-room cwd.
	// biome-ignore lint/correctness/useExhaustiveDependencies: setDefaultShell/setDefaultCwd are plain useState setters passed in from App.tsx (#19) — stable across renders, but biome can't prove that through a function parameter.
	useEffect(() => {
		void invoke<string[]>("default_shell").then(setDefaultShell);
		void invoke<string>("default_cwd").then(setDefaultCwd);
	}, []);

	// #185: every quit path runs through this — window close button
	// (onCloseRequested) and the macOS Cmd+Q menu item, which lib.rs
	// deliberately routes here as skein://quit-requested because the
	// predefined quit item would terminate: without any prompt.
	// destroy() bypasses onCloseRequested, so confirming quits for
	// real; the ref guard stops stacked prompts when close is clicked
	// twice.
	const quitPromptOpenRef = useRef(false);
	useEffect(() => {
		const confirmQuit = async (): Promise<boolean> => {
			const dirty = filesRegistry.anyDirty();
			if (dirty.length === 0) return true;
			if (quitPromptOpenRef.current) return false;
			quitPromptOpenRef.current = true;
			try {
				const shown = dirty.slice(0, 5).join(", ") + (dirty.length > 5 ? ", …" : "");
				return await confirm(
					`${dirty.length} unsaved file${dirty.length === 1 ? "" : "s"} (${shown}) will be discarded. Quit anyway?`,
					{ title: "Unsaved changes", kind: "warning" },
				);
			} finally {
				quitPromptOpenRef.current = false;
			}
		};
		const unClose = getCurrentWindow().onCloseRequested(async (event) => {
			// Fail OPEN: any throw in here must still end in the window
			// closing — 0.2.6 shipped unclosable when the wrapper's
			// destroy was capability-denied (#196). Losing unsaved text
			// beats an app you cannot exit.
			try {
				if (filesRegistry.anyDirty().length === 0) return;
				event.preventDefault();
				if (await confirmQuit()) void getCurrentWindow().destroy();
			} catch (err) {
				console.error("[skein] close-requested handler failed; closing anyway:", err);
				void getCurrentWindow().destroy();
			}
		});
		const unQuit = listen("skein://quit-requested", () => {
			void confirmQuit()
				.then((ok) => {
					if (ok) void getCurrentWindow().destroy();
				})
				.catch((err: unknown) => {
					console.error("[skein] quit handler failed; closing anyway:", err);
					void getCurrentWindow().destroy();
				});
		});
		return () => {
			void unClose.then((f) => f());
			void unQuit.then((f) => f());
		};
	}, []);

	// #189: Esc dismisses the picker. The picker hides every body, so
	// the (display:none'd) xterm can't swallow the key — a plain window
	// listener receives it.
	// biome-ignore lint/correctness/useExhaustiveDependencies: setShowPicker is a plain useState setter passed in from App.tsx (#19) — stable across renders, but biome can't prove that through a function parameter.
	useEffect(() => {
		if (!showPicker) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape") {
				e.preventDefault();
				setShowPicker(null);
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [showPicker]);

	// Phase 4: listen for the macOS app menu's Preferences… item.
	// lib.rs's on_menu_event emits skein://open-settings when the user
	// picks Skein → Preferences… from the menu bar; we open the same
	// modal as the cog icon and Mod+,.
	// biome-ignore lint/correctness/useExhaustiveDependencies: setShowSettings is a plain useState setter passed in from App.tsx (#19) — stable across renders, but biome can't prove that through a function parameter.
	useEffect(() => {
		const promise = listen("skein://open-settings", () => setShowSettings(true));
		return () => {
			void promise.then((un) => un());
		};
	}, []);

	// #132: shared hover popover for status dots / harness chips (replaces
	// the native title=). One delegated listener for the whole app.
	useEffect(() => attachStatusPopover(), []);

	// #120: this window-level swallow predates #271. Back when
	// `dragDropEnabled` was `false` (kept off so the in-webview
	// harness-reorder DnD of #26 would work), a stray OS file drop hit
	// the webview's default handler and navigated to the `file://` URL —
	// a black screen with no way back. #271 rebuilt tab reorder on
	// pointer events precisely so `dragDropEnabled: true` could go on —
	// Tauri's native handler now owns OS file drops before the webview
	// ever sees them, which should make this redundant. It stays as
	// belt-and-braces until that's verified on both macOS and Windows
	// (can't be done from here); #41 will consume real file drops via
	// `getCurrentWebview().onDragDropEvent` rather than this listener.
	// Gated to actual file drags (`dataTransfer` carries "Files") so it
	// doesn't interfere with anything else.
	useEffect(() => {
		const swallow = (e: WindowEventMap["drop"]) => {
			if (e.dataTransfer?.types?.includes("Files")) e.preventDefault();
		};
		window.addEventListener("dragover", swallow);
		window.addEventListener("drop", swallow);
		return () => {
			window.removeEventListener("dragover", swallow);
			window.removeEventListener("drop", swallow);
		};
	}, []);
}
