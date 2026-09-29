# Design surface — recon spike (#431, epic #430)

Slice 0 of the design surface epic. It answers the epic's four open
questions with evidence, so slices 1–3 can be written from facts.
Nothing here is production code. The throwaway probes described below
lived in a scratch folder and are not committed.

**Summary**

- **Rendering works.** A real prototype renders in full inside
  `<iframe sandbox="allow-scripts">` in a Tauri 2.11.0 release build,
  on both Windows (WebView2) and macOS (WKWebView). It is served over
  HTTP from `127.0.0.1`, and reloading on a changed URL works. One
  header is mandatory: `Access-Control-Allow-Origin: *`. Without it the
  page renders blank with no error. That was observed in Chrome and in
  WKWebView; WebView2 was only run with the header on.
- **Picking works.** A script injected by the server reports the
  clicked element to the host over `postMessage`: selector, text,
  attributes, rect, and a JSX source file and line. Only one source
  file was sampled.
- **The iframe cannot reach Tauri IPC.** On Windows it sees
  `__TAURI_INTERNALS__`, but every `invoke` from it is refused. On
  macOS the object is not injected into the iframe at all.
- **Element threads fit the review model, with changes.** They need a
  sibling anchor table rather than new columns, and a DOM-side
  re-anchoring pass under the same never-silently-moved-or-dropped
  contract. Sign-off covers them unchanged.
- **Open Design is a reference, not a library.** Borrow its conventions
  (`data-od-id`, the anchor state ladder). Lift none of its code.
- **No platform split.** One serving model works on both OSes (§2.6).

## 1. How the prototypes load

The first user is a real-world `design/` folder of HTML/JSX prototypes
and tokens. It was inspected read-only.

- **Entry.** There is one entry HTML file in that folder's `prototypes/`
  directory. It holds a `<link rel="stylesheet">`, a `<div id="root">`,
  three CDN scripts and 16 local scripts: 3 plain `.js` and 13 `.jsx`.
  The probes loaded each page twice (first load, then the reload
  check), hence 26 `.jsx` requests in §2.2.
- **JSX is transpiled in the browser.** The page loads
  `@babel/standalone` 7.29.0, and the JSX modules are
  `<script type="text/babel" src="proto/*.jsx">`. There is no bundler
  and no build step. The folder's `package.json` exists only for its
  Node test suites. Babel fetches each `text/babel` source with **XHR**,
  which matters for §2.2.
- **Tokens are a script, not a stylesheet.** `proto/tokens.js` defines a
  global token object. Components call it for paint values, and it
  writes CSS custom properties onto the document at load time and on
  theme change.
- **CDN dependencies:** React 18.3.1 and ReactDOM 18.3.1 (the UMD
  *development* builds) and `@babel/standalone`, all from unpkg with SRI
  `integrity` and `crossorigin="anonymous"`. **Rendering needs
  network access**; §2.7 covers going offline.
- **Paths are all relative** (`proto/…`), and there are no root-relative
  `/…` paths. Load order is significant: data and tokens load first,
  then the JSX modules, which share state through `window` globals.
- **Stable element ids already exist.** Screens and controls carry
  `data-od-id="…"` (95 of 742 rendered elements), screen roots carry
  `data-screen-label`, and wallpaper carries `data-*`. These come from
  the design tool's authoring convention (§4). They are the best anchor
  material available, and they cost Skein nothing.

**Implication for "any HTML entry file"**: the renderer must be a plain
static file server over the room's worktree. It must not change how the
page loads, and it adds only a picker script. Nothing in that is
specific to this one folder.

## 2. Rendering in Skein

### 2.1 What was tested

The probes ran on a Windows 11 box and were three things:

- A scratch Node HTTP server serving the prototypes folder under
  `/preview/room1/`, with toggles for CORS and for rewriting the Babel
  `<script>` tags. It injected `<script src="/__picker.js">` before
  `</head>` and logged beacons to a file, so no pixels were needed.
- A host page on a different origin, holding
  `<iframe sandbox="allow-scripts" src="http://127.0.0.1:<port>/preview/room1/<entry>.html">`.
- The same host page in two places: headless Chrome, and a **minimal
  throwaway Tauri app pinned to 2.11.0** (Skein's locked version) built
  with `cargo build --release`. The Tauri app had `csp: null`, default
  capabilities and no `useHttpsScheme`, the same as Skein's
  `tauri.conf.json`. Its host origin was `http://tauri.localhost`, the
  production origin on Windows.

### 2.2 Results

| Case | Renders (elements / `data-od-id`) | Notes |
|---|---|---|
| Chrome, sandboxed, no CORS header | **0 / 0**, blank | Babel's XHR for the `.jsx` files is blocked from the opaque origin. **No error** reaches `onerror` or the console. Not re-run under WebView2. |
| Chrome, sandboxed, `ACAO: *` | 742 / 95 | Only Babel's "in-browser transformer" warning. |
| Chrome, no sandbox (control) | 742 / 95 | Same output; ACAO not needed. |
| **Tauri 2.11 release, WebView2**, sandboxed, `ACAO: *` | **742 / 95** | Host `http://tauri.localhost`; `postMessage` received with `e.origin === "null"`, `e.source === iframe.contentWindow`. |
| **Tauri 2.11 release, WKWebView** (macOS 26.2), sandboxed, `ACAO: *` | **742 / 95** | Host `tauri://localhost`; no mixed-content or blocked-frame error; same `postMessage` result; `_debugSource` identical to Windows. |
| Tauri 2.11 release, WKWebView, sandboxed, no CORS header | **0 / 0**, blank | Same silent failure as Chrome: the `.jsx` requests reach the server with `Origin: null`, and nothing is surfaced. |

- **SRI from an opaque origin works.** The CDN tags use
  `crossorigin="anonymous"` and unpkg sends `ACAO: *`.
- **Reload works.** Setting `iframe.src` to `…/entry.html?v=2` produced
  a second full render and a second picker message, and the server log
  shows every sibling `.jsx` re-fetched (with `Cache-Control: no-store`).
  `contentWindow.location.reload()` is not available: the frame is
  cross-origin.
- **The iframe cannot reach Tauri IPC.** Tauri's init scripts run in
  every frame, so `window.__TAURI_INTERNALS__` exists *inside* the
  sandboxed iframe (`window.__TAURI__` does not, because
  `withGlobalTauri` is off). Every `invoke` from the iframe was refused
  before any command ran: a custom command, `plugin:app|version` and
  `plugin:event|listen` all failed with `Origin header is not a valid
  URL`, on both first load and reload. The custom command's own log
  shows no call from the iframe. The same three calls succeed from the
  host. This matches the fix for CVE-2024-35222
  (GHSA-57fm-592m-34r7): iframes lost IPC in 2.0.0-beta.20, except on
  Windows when the iframe shares the window's origin, and a null origin
  never does. On macOS the refusal is stronger: `__TAURI_INTERNALS__` is
  `undefined` inside the sandboxed iframe, and the custom command's log
  shows only the host's call.

### 2.3 Recommended serving model

- **Serve from localhost HTTP, from axum.** The iframe origin is then
  `http://127.0.0.1:<port>` on every OS, which gives the fewest
  platform differences. Relative paths, Babel's XHR and CDN loads are
  all plain HTTP. The alternatives were rejected on evidence and
  research:
  - **The asset protocol** has a per-OS origin (`asset://` or
    `http://asset.localhost`). Nothing confirms that relative XHR
    works under it.
  - **A custom URI scheme** also has a per-OS URL form, and has no
    confirmed iframe or CORS behaviour.
  - **Tauri multi-webview** is still behind the `unstable` feature, and
    a native child webview cannot sit inside React's layout.
- **Mount the preview router on the existing agent-API server**
  (`app/src-tauri/src/agent_api/http.rs`), or on a second listener with
  the same bind pattern (`lib.rs`, `127.0.0.1:0`). A second listener
  keeps preview CORS headers off `/api` and `/mcp` entirely. Slice 1
  decides; either works.
- **The route needs its own capability, not the bearer token.** An
  iframe cannot send an `Authorization` header, and agent-API auth is
  per-route (`with_caller`), so a preview route is unauthenticated by
  default. Use an unguessable per-room path segment,
  `/preview/<random-token>/…`, minted like the room token and revoked
  on boot. Resolve every path under the canonical worktree root of *the
  token's own room*, and refuse `..` and symlinks that escape it. Do
  **not** reuse `fs.rs`'s `ensure_room_scope` as it stands: it accepts
  a path under *any* persisted room's folder (the #174 gap), so room
  A's preview token would serve room B's files.
- **Response headers that worked:**
  - `Content-Type`: `text/javascript` for `.js`/`.jsx`, `text/html`,
    and `text/css`, each with `charset=utf-8`.
  - `Cache-Control: no-store`.
  - `Access-Control-Allow-Origin: *`, on preview routes only.
- **Picker injection:** the server rewrites HTML responses to add one
  `<script>` before `</head>`. A page with its own meta CSP could block
  it. None of the real prototypes has one, so slice 1 detects it and
  reports it rather than fighting it.

### 2.4 Iframe and host rules

- **`sandbox="allow-scripts"`, and never `allow-same-origin`.** This
  rule is conservative. Only the null origin was probed. With
  `allow-same-origin` the frame would get the real origin
  `http://127.0.0.1:<port>`, which is not the host's, so neither the
  Windows same-origin IPC exception nor a sandbox escape follows
  directly. But a real origin moves the IPC refusal from "Origin is not
  a valid URL" to the capability scope, and that was not tested. The one
  cost is that the host cannot touch `contentDocument`, so everything
  goes over `postMessage`.
- **Picker messages are untrusted input.** The picker runs in the
  prototype's own page context, and the prototype is arbitrary worktree
  code, so any message can be forged. The host and the Rust write
  command validate the shape, cap the sizes, and render every field as
  text, never as HTML.
- **Host listener:** accept a message only when
  `e.source === iframe.contentWindow` (origin is `"null"`, so don't
  check it), and post to the frame with target `"*"`.
- **Skein's CSP is `null`** (`tauri.conf.json`), so an iframe needs no
  CSP change today. If Skein ever sets a CSP, it must include
  `frame-src http://127.0.0.1:*`.

### 2.5 Reload on a watcher tick

This is feasible with existing parts. `WatcherManager::start` or
`start_with_paths` (`app/src-tauri/src/watcher.rs`, 200 ms debounce)
already emits ticks over a Channel. The per-room #221 discovery watcher
already runs over the same worktree. The design pane:

- bumps `?v=n` on the entry URL whenever a tick's paths touch any
  worktree file outside `.git/` and `node_modules/`. Shared tokens and
  assets often sit outside the entry's directory (`../`). A narrower
  trigger would use the set of files the server actually served for the
  current load;
- keeps the current selection host-side and re-resolves it from the
  anchor after the reload (§3).

A reload costs one full Babel re-transpile. That is fine at this size,
but worth watching on larger prototypes.

### 2.6 macOS WKWebView

The open question was whether WebKit treats an `http://127.0.0.1`
iframe under the `tauri://localhost` custom-scheme parent as mixed
content. **It does not.**

The same throwaway host app (Tauri pinned to 2.11.0, release) was
rebuilt and run on macOS 26.2 (WebKit 26.2) with the same server and
picker beacons. The results are the macOS rows in §2.2:
- a full render with no mixed-content or blocked-frame errors;
- `postMessage` delivered;
- `?v=2` reload with every sibling refetched;
- the same `_debugSource`;
- no IPC object in the iframe.

The app window opened directly from an ssh launch into the logged-in
session. **No macOS-only fallback is needed.** The localhost serving
model in §2.3 holds on both platforms.

### 2.7 Offline and CDN

Prototypes that load React and Babel from a CDN will not render
offline. Skein should not fix that silently, for example by vendoring
copies or rewriting URLs, because the page must render exactly as the
design folder defines it. Slice 1 reports failed script loads in the
pane, from picker beacons of `error` events on `<script>`.

## 3. Element anchoring and the review model

### 3.1 What an element anchor holds

The anchor is captured by the picker at comment time. It is evidence,
never rewritten, the same rule `anchor_lines` follows today:

```json
{
  "entry": "prototypes/<entry>.html",
  "odId": "rail-group-first-run",
  "selector": "#root > div:nth-of-type(1) > div:nth-of-type(3) > button:nth-of-type(1)",
  "tag": "button",
  "text": "First run",
  "attrs": { "class": "rgroup-h", "aria-expanded": "false" },
  "source": { "file": "prototypes/proto/shell.jsx", "line": 16, "column": 9 },
  "rect": { "x": 41, "y": 197, "w": 235, "h": 27 }
}
```

- **`odId`** is the strongest key when it exists (`data-od-id`, else
  `data-screen-label`, else the element's own `id`).
- **`selector`** is an `nth-of-type` path. It breaks silently on
  reorder, so it is never trusted alone.
- **`source`** comes from React's development fiber (`_debugSource`)
  once Babel runs the `transform-react-jsx-source` plugin. The preview
  server adds it by rewriting each `type="text/babel"` tag with
  `data-plugins="transform-react-jsx-source"`. The prototype is
  unchanged on disk. In the probe, `_debugSource` sat on the picked
  element's own fiber and gave a correct line.
  - Babel reports `fileName` as a mangled URL
    (`/http:/127.0.0.1:<port>/preview/<room>/proto/shell.jsx`), so the
    host strips the server prefix to get a worktree-relative path.
  - This only works on React 18 with the development build: React 19
    removed `_debugSource`. Plain HTML has no source mapping at all, so
    `source` is optional, and a Rust-side text search is its fallback
    (§3.3).
  - Unverified: both sampled picks came from `shell.jsx`. Slice 2 must
    confirm that elements from other `.jsx` files report their own
    file.

### 3.2 Storage: a sibling table, not new columns

`review_threads` (`app/src-tauri/src/db.rs:628-642`) is
`CREATE TABLE IF NOT EXISTS`, and **Skein has no migration step** (no
`ALTER TABLE` or `user_version` anywhere). A new column would never
appear in an existing database. New per-thread state has always gone
into sibling tables, such as `review_addressed`. So:

- **The thread row is unchanged.** It gets `scope = 'element'`, a new
  `thread_scope` constant beside `LINE`/`FILE`/`COMMIT`/`REVIEW` in
  `review_surface.rs:81-84` and accepted by `write.rs`'s scope check.
  `file_path` holds the **entry HTML** path, and the line, side and
  anchor columns are `NULL`. All of those are already nullable.
- **A new table** holds the anchor:
  `review_element_anchors(thread_id PK, room_id, file_path,
  anchor_json, last_seen_json, updated_ms)`. `anchor_json` is the §3.1
  evidence and is written once. `last_seen_json` is the placement the
  design pane last computed: state, resolved selector and rect.
- **The table must join `ROOM_KEYED_TABLES`** (`db.rs:365`), or
  `sweep_orphans` (#237) never cleans it. The existing length-check
  test enforces this.
- **`review_comments` needs no change.** Replies, authors and
  addressed marks are anchor-agnostic.

### 3.3 Re-anchoring under the same contract

`crates/skein-review/src/anchor.rs` matches line windows of text. It
cannot take a DOM, and a CSS path is not a line slice. **The contract
carries over; the code does not.**

- **A new pure matcher, `elementAnchor.ts`,** runs in the design pane,
  because only the iframe has the DOM. It is table-tested like
  `anchor.rs`'s 22 cases. Its tiers mirror `anchor.rs`, and the state
  names match Open Design's ladder (§4):
  1. **exact:** the same `odId`, or the same selector with the same tag
     and text → `anchored`.
  2. **found elsewhere:** a unique element with the same `odId`, text
     or attributes → `reanchored`, with the new position recorded.
  3. **best structural overlap** above a threshold → `stale`, which
     renders as a guess.
  4. **nothing** → `lost`. The thread renders pinned at its last-seen
     rect, with its original evidence shown, the way outdated line
     threads render above the diff today.
- **The picker runs the matcher inside the iframe** on request, since
  the host can't reach the DOM. It returns results over `postMessage`.
- **Write-back** goes through one new command that updates
  `last_seen_json` and never touches `anchor_json`. That mirrors
  `anchoring.rs`, which writes a line position back and never the
  anchor text.
- **`last_seen_json` carries a content stamp:** a hash of the entry
  file plus every file served for that load. A placement whose stamp
  no longer matches the files on disk is reported as stale or unknown,
  never as `anchored`. Otherwise an edit made while the pane was
  closed would leave the old position silently claimed.

**The agent side has no DOM.** `list_comments` and `get_comment` build
their view in Rust (`agent_api/verbs.rs`, via
`review_surface::query::file_impl`). For an element thread they return:

- the stored anchor;
- the last pane-computed state and its timestamp;
- the source lines the element maps to, as "the code it is about".
  These come from `source` when present, else from a Rust-side search
  of the worktree for the `odId` or the text snippet. They are labelled
  `placement: "source_guess"` and flagged outdated unless the pane last
  reported `anchored` *and* that report's content stamp still matches
  the files on disk.

**Rust never claims `unmoved` for an element thread on its own.** The
agent can be told "unknown", never a silently wrong position. The #328
`request_frontend` round-trip could do a live re-anchor, but it fails
while the pane is closed, so it is optional.

### 3.4 Scopes, the review pane and sign-off

- **Diff scopes don't change.** A thread's `scope` column describes its
  kind, not a diff view. `file` threads already show how a
  non-line thread works:
  - `ScopeFiles::load` groups threads by `file_path`;
  - `file_impl` returns a file's threads even when the file has no
    diff.
  So an element thread on an unchanged entry file still appears in the
  review pane's file list.
- **What must change:**
  - `to_thread_dto` in `review_surface/anchoring.rs` must branch on
    `element`, or the thread is reported `unmoved`.
  - `ThreadDto` gains an optional element block.
  - The review pane renders an element thread as its anchor summary
    (id, text and source line), with a "show in design pane" link.
- **Agent verbs:** `list_comments` filters only by status and file, so
  element threads are not hidden. `scope: "element"` is a new value
  there, and `docs/agent-api.md` and the `AgentThread` doc must list
  it. `get_comment`'s `diff_context` is already safe: with no
  `line_start` it yields nothing rather than crashing. It still runs
  `file_impl` against the entry HTML for no result, so the element
  branch skips that call.
- **Sign-off needs no change.** `review_surface/signoff.rs` counts
  every unresolved row in `review_threads` for the room, with no filter
  on scope or file. It uses `review_addressed` for the addressed count.
  Element threads are therefore counted in `review_status` from day
  one. Sign-off stays keyed on `head_sha`, so a prototype is reviewed
  as committed work, exactly like code.

### 3.5 Verdict

**It fits, with changes.** The working assumption holds: element
comments as `review_threads` get the review pane, the agent verbs and
sign-off almost for free. The costs are:

- a sibling anchor table rather than a column;
- a DOM-side matcher with its own tests;
- an element branch in three Rust places: thread DTO, agent thread,
  `get_comment`.

## 4. Reuse from Open Design

Open Design (Apache-2.0, v0.24.1) was read at the source level; nothing
of it is embedded.

- **Worth borrowing as conventions** (rewritten, not copied):
  - **the `data-od-id` authoring convention.** The real prototypes
    already use it.
  - **its fallback for elements without an id.** It stamps positional
    ids at render time; this recon keeps the selector plus text instead,
    because positional ids re-point silently on reorder.
  - **its comment-target shape:** element id, selector, label, text,
    position and an HTML hint.
  - **its anchor ladder:** `anchored` / `reanchored` / `stale` /
    `lost`, under a rule that drift is "never silently mis-pointed".
    That is the same contract as Skein's.
  - **its sandbox choice:** `allow-scripts` without `allow-same-origin`,
    over `postMessage`.
- **Not worth lifting:**
  - **its picker.** It is a ~1,200-line inline string inside a
    4,449-line render module, coupled to its deck, export and palette
    bridges.
  - **its design-system extraction.** It lives inside its daemon, bound
    to its contracts package, and outputs `manifest.json`, `DESIGN.md`
    and `tokens.css`.
- **Not applicable:** **its MCP server** has 22 tools and none for
  comments, which matches the pilot. Skein's own review verbs already
  cover the loop.
- **Licence:** Apache-2.0. If a file is ever copied, keep its header
  and the licence text and mark the changes. The plan above copies
  nothing.

## 5. Open items for the slices

| Item | Where it gets answered |
|---|---|
| `_debugSource` file names for elements from other `.jsx` files | slice 2, with the picker in place |
| Preview route on the agent-API listener or its own | slice 1 |
| Babel re-transpile time on large prototypes | slice 1, by measuring reload latency |
| Choosing which HTML file is the entry | slice 1 (a picker over `*.html` in the worktree; remembered per harness) |

## 6. Draft issues for slices 1–3

These are drafts for review, not filed. Each one names its epic (#430)
and this recon.

### Slice 1 — `design` harness kind: render a worktree HTML entry and reload on change

A non-PTY harness kind, like `files` (#184/#185), that renders one HTML
entry file from the room's worktree in a sandboxed iframe.

- **Registry.** `design` joins `HARNESS_KINDS` (`app/src/data.tsx`) with
  `pty: false`, the Rust `HarnessKind` enum, and its
  `agrees_with_the_typescript_registry` test. `HarnessColumn.tsx`'s
  non-PTY branch picks `DesignBody` for it instead of `FilesBody`.
- **Preview server.** It lives on the agent-API listener or a second
  `127.0.0.1:0` listener (decide and record why). Routes are
  `/preview/<per-room token>/<path>`. The token is minted per room and
  revoked on boot. Every path is canonicalized under *that room's*
  worktree; `..` and escaping symlinks are refused, and
  `ensure_room_scope` is not reused (§2.3). Responses carry the
  correct MIME type, `Cache-Control: no-store` and
  `Access-Control-Allow-Origin: *`. HTML responses get the picker
  `<script>` injected before `</head>`, and `type="text/babel"` tags
  get `data-plugins="transform-react-jsx-source"`.
- **Pane.** An entry picker over the worktree's `*.html` files, stored
  on the harness record as a `#[serde(default)]` field.
  `<iframe sandbox="allow-scripts">`, never `allow-same-origin`.
  Failed script loads (from picker beacons) show in the pane, which is
  the offline and CDN case.
- **Reload.** On a room watcher tick touching any worktree file
  outside `.git/` and `node_modules/`, bump `?v=n`.
- **Acceptance:**
  - the real design folder's prototype renders in the pane on Windows,
    and on macOS;
  - saving a `.jsx` reloads it;
  - a preview URL for room A cannot read a file of room B;
  - an `invoke` from the iframe is refused (a test page in a fixture
    worktree).

### Slice 2 — element-anchored comments as review threads

Pick an element in the design pane, comment on it, and see the thread
in the design pane and in the review pane.

- **Storage.** A new `review_element_anchors(thread_id PK, room_id,
  file_path, anchor_json, last_seen_json, updated_ms)` table, added to
  `ROOM_KEYED_TABLES`. `thread_scope::ELEMENT = "element"` is accepted
  by `write.rs`. The thread row gets `file_path` = the entry HTML and
  NULL line columns (§3.2).
- **Picker.** It captures `odId`, the selector, tag, text, attributes,
  rect and the JSX `source` (prefix-stripped to a worktree path).
  Every message is validated and size-capped host-side, and rendered as
  text.
- **Matcher.** A pure `elementAnchor.ts` with tiers `anchored` /
  `reanchored` / `stale` / `lost`, table-tested in the style of
  `anchor.rs`. It never silently moves or drops a thread. A `lost`
  thread pins at its last-seen rect with its original evidence.
- **Write-back** goes to `last_seen_json` only, with a content stamp of
  the files served; `anchor_json` is never rewritten.
- **Review pane:** `to_thread_dto` gets an element branch, so it is
  never reported `unmoved`. The thread renders its id, text and source
  line, with a "show in design pane" link.
- **Acceptance:**
  - a comment survives a reload and an edit that reorders siblings;
  - deleting the element makes the thread `lost` and visible, not
    gone;
  - the thread is counted in the sign-off counts;
  - it is confirmed that picks from `.jsx` files other than the first
    report their own `source` file.

### Slice 3 — the agent loop over element threads

Agents read element threads, reply and mark them addressed through the
existing verbs, and sign-off covers them.

- **`list_comments` and `get_comment`** return `scope: "element"`,
  the stored anchor, and the last pane-computed state with its stamp
  and timestamp. "The code it is about" is the source lines, from
  `source` or a Rust-side search for the `odId` or text. These are
  labelled `placement: "source_guess"` and flagged outdated unless the
  last pane state was `anchored` with a current stamp (§3.3). Rust never
  claims `unmoved` on its own.
- **Skip `diff_context`** for element threads.
- **Update `docs/agent-api.md`** and the `AgentThread` doc for the new
  scope.
- **Acceptance:**
  - in a fixture room, an agent sees an element thread with its source
    lines;
  - `reply` and `mark_addressed` round-trip into both panes;
  - `review_status` counts it;
  - after an edit the pane has not re-rendered, the agent gets
    "stale", not the old position.

### Slice 4 — proposed edits: change the design in the pane, the agent writes the source

Added after the recon, by decision. The reviewer can drag, nudge,
restyle and retype elements in the design pane. Skein does **not** write
those changes to source. Each change becomes a precise **change
request** on an element thread, and the agent applies it through the
normal review loop. The source stays authoritative, and every change is
reviewed like code.

Why proposals and not direct writes: the page is generated by React
from hand-written JSX. A moved box on screen has no single correct
source edit: it could be a `style` prop, a CSS rule, a token or layout
code. The agent can judge that; a generic write-back cannot. True
WYSIWYG editing, where Skein writes source itself, is out of scope for
this epic.

- **Edit mode in the picker bridge:**
  - drag and resize handles, and arrow-key nudging at 1 px, or 8 px
    with Shift;
  - a small property panel: size, spacing, colour, font, radius and
    opacity;
  - inline text editing through `contenteditable` on the picked
    element's text.
  Changes preview live in the iframe as inline style or text overrides.
  Nothing on disk changes.
- **The change request is structured.** It holds the §3.1 element
  anchor plus a list of `{ property, from, to }`, with `from` read from
  computed style before the edit. A drag is recorded as an offset
  (`dx`/`dy`), not a guessed CSS property, and the agent decides
  whether that means margin, gap, position or layout.
  - When a new value equals a design token's value (tokens are exposed
    as CSS custom properties), the request names the token, so the
    agent writes the token rather than a raw value.
  - Text edits carry the old and new string plus the JSX `source`
    line, so the agent can find the literal.
- **Storage.** It is added to slice 2's new table as `proposal_json`,
  nullable, so no migration is needed. The thread body is a readable
  summary generated from it, for example "padding-left 12px → 16px
  (token space.4)".
- **Pending proposals stay visible.** After a reload the pane re-applies
  unaddressed proposals as a dashed "proposed" overlay. Once the agent
  marks one addressed and the reload shows the change in the real
  render, the overlay drops away. If the matcher can't place the
  element, the proposal renders as `lost`, never silently discarded.
- **Agent side.** `get_comment` returns `proposal` as structured data
  alongside the source lines (slice 3). `docs/agent-api.md` documents
  the shape. The agent replies and marks it addressed as usual.
- **Batching.** Several edits to one element before saving make one
  thread. A "propose" action commits the batch, so there is no thread
  per keystroke.
- **Out of scope:** Skein writing source edits itself, and structural
  edits (adding, deleting or reparenting elements). Those stay ordinary
  comments.
- **Acceptance:**
  - nudging a button 4 px and changing its text yields one thread with
    a structured proposal;
  - an agent applies it and marks it addressed, and after the reload
    the overlay is gone and the real render matches;
  - a value that equals a token is proposed as that token;
  - reloading before the agent acts keeps the proposal visible;
  - a forged picker message with an oversized or malformed proposal is
    rejected.
