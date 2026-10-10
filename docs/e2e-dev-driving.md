# Driving a dev Skein from an agent (Windows)

Verified end to end on 2026-10-10.

## 1. Isolate the instance

- Single-instance forwards a second launch to the running Skein and
  exits, so a second app on an identifier already in use never starts.
- Two Skeins on one `skein.db` erase each other's rooms (`save_all`
  replaces every row).
- Dev's vite port 1420 is `strictPort` and may belong to another
  worktree.

So run an isolated profile. From `app/`:

    npm run tauri:dev -- --config <scratch>/e2e.conf.json

The override file is **untracked**, kept in a scratch dir, never
committed. The window object is copied from `app/src-tauri/tauri.conf.json`:

    {
      "productName": "Skein (e2e)",
      "identifier": "com.timeloop-vault.skein.e2e",
      "build": {
        "beforeDevCommand": "npm run dev -- --port 1421",
        "devUrl": "http://localhost:1421"
      },
      "app": {
        "windows": [{
          "label": "main", "title": "Skein", "width": 1320, "height": 820,
          "minWidth": 900, "minHeight": 600, "decorations": true,
          "titleBarStyle": "Overlay", "hiddenTitle": true,
          "resizable": true, "center": true, "visible": true,
          "dragDropEnabled": true,
          "additionalBrowserArgs": "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --remote-debugging-port=9223"
        }]
      },
      "plugins": { "deep-link": { "desktop": { "schemes": ["skein-e2e"] } } }
    }

- `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` does **not** work: wry sets
  the args itself. Use `additionalBrowserArgs`.
- `additionalBrowserArgs` replaces Tauri's defaults, hence the copied
  `--disable-features`.
- The `--config` file overrides `tauri.conf.json`; the window array is
  replaced, not patched, so it must be complete.

## 2. Drive the UI over CDP

- Targets are listed at `http://127.0.0.1:9223/json`. The main target is
  the devUrl page; each design preview iframe is its own target.
- Use node with the `ws` package and `Runtime.evaluate`.
- React-controlled inputs: set the value through the native
  `HTMLInputElement.prototype` value setter, then dispatch a bubbling
  `input` event. Plain `.value =` is ignored by React.
- Buttons: find by `innerText` and `.click()`.
- Keys and terminal text: `Input.dispatchKeyEvent` / `Input.insertText`.
- New Room's folder field accepts a typed path.
- `Page.captureScreenshot` works only on the top-level target.

Example:

    const WebSocket = require("ws");
    const targets = await (await fetch("http://127.0.0.1:9223/json")).json();
    const main = targets.find((t) => t.type === "page" && t.url.includes("1421"));
    const ws = new WebSocket(main.webSocketDebuggerUrl);
    await new Promise((r) => ws.once("open", r));
    ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: {
      expression: "document.title", returnByValue: true } }));
    ws.on("message", (m) => { console.log(JSON.parse(m)); ws.close(); });

## 3. Call the agent API

The URL and token exist only in a harness's environment.

- Add a shell harness in the e2e room; have it write
  `$env:SKEIN_REVIEW_URL` and `$env:SKEIN_REVIEW_TOKEN` to a scratch
  file.
- Call `/api/*` or `/mcp` with `Authorization: Bearer <token>` (see
  `docs/agent-api.md`). MCP is stateless: `initialize` needs no session
  header.
- Never print the token. Delete the scratch file afterwards.
- Design control verbs (`open_design_entry`, `set_design_device`,
  `show_element`, `invoke_element`, `show_changes`,
  `get_design_screenshot`, `show_design_pane`) share a limit of 10
  calls per calling room per minute; refused attempts count too. Reads
  (`list_design_harnesses`, `get_design_state`) are not limited.

## 4. Gotchas

- The dev exe is `skein-app.exe`, the same name as the release. Stop
  processes and pick windows only by a captured PID, or by a parent
  chain under this worktree's `target\debug`; never by name or title.
- TaskStop on a background command can orphan cargo or test children,
  so check by PID that they are gone.
- A stale localStorage `skein:harnessColWidth` from a bigger monitor can
  push the right pane off-screen. Clear it via CDP.

## 5. Cleanup

- Stop your own process tree by PID, including the `msedgewebview2`
  children of the e2e profile.
- Delete `%APPDATA%\com.timeloop-vault.skein.e2e` and
  `%LOCALAPPDATA%\com.timeloop-vault.skein.e2e`.
- Delete the registry key `HKCU\Software\Classes\skein-e2e`.

## 6. Open gap

macOS is not covered: WKWebView has the Web Inspector rather than CDP.
