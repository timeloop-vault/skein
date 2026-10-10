//! The four design-pane tool specs (#512), split out of `mcp.rs` to keep
//! that file under the size cap. `tool_specs()` splices these in, in order.

use serde_json::{Value, json};

pub(super) fn design_tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_design_harnesses",
            "title": "List this room's design harnesses",
            "description":
                "Every design harness in your own room: id, name, the current \
                 entry file (null when unset), the device setting as stored \
                 (null if never set), the entry files available to open_design_entry, \
                 and whether its pane is mounted and ready. mounted and ready are \
                 null when Skein's window cannot answer right now. Read-only; takes \
                 no focus.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "get_design_state",
            "title": "Read one design pane's live state",
            "description":
                "The live state of a design pane in your own room: entry (null \
                 when unset), device, ready, loadFailed, errors (script errors the \
                 preview reported), selected (the selected element as an element \
                 anchor, or null) and scroll. The pane must be mounted (open in \
                 Skein, not necessarily visible), otherwise this refuses with \
                 not_mounted. A preview that has not loaded is not a refusal: you \
                 get ready: false (and loadFailed). `harness` is optional when the \
                 room has exactly one design harness (no_design_harness / \
                 harness_required otherwise). Read-only; takes no focus.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": { "type": "string", "description": "A design harness id." },
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "open_design_entry",
            "title": "Switch a design harness's entry file",
            "description":
                "Sets the entry (the HTML file the pane previews) of a design \
                 harness in your own room, persisting it the way the toolbar picker \
                 does. Works whether or not the pane is mounted. `entry` must be \
                 exactly one of the entries list_design_harnesses returns, otherwise \
                 unknown_entry (with the available ones). Answers with the new and \
                 previous entry. Never takes focus or switches the visible room or \
                 harness. Gated by the Settings switch that also governs \
                 open_harness/close_harness, and rate-capped separately (10 combined \
                 open_design_entry/show_element calls per minute; the cap is checked \
                 before the harness and entry are validated, so refused attempts \
                 count too).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "entry": {
                        "type": "string",
                        "description": "A worktree-relative, /-separated entry path from list_design_harnesses.",
                    },
                },
                "required": ["entry"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "set_design_device",
            "title": "Set or clear a design pane's device preview",
            "description":
                "Sets (an object) or clears (null) the device setting of a design \
                 harness in your own room — the viewport the preview is framed in — \
                 persisting it the way the toolbar does. null means no device: the \
                 pane fills, host pixel ratio, no touch. Works whether or not the \
                 pane is mounted. `device` is required. Fields: preset (\"none\", \
                 \"custom\", \"iphone-16-pro\", \"iphone-14\", \"android\", \
                 \"ipad-air\"); width and height (integers 100-4000, only with \
                 \"custom\", both required there); landscape (boolean, not with \
                 \"none\"); dpr (0.5-4); touch (boolean: mouse drag acts as touch, \
                 and the page sees hover: none / pointer: coarse). Unknown keys and \
                 out-of-range values are refused with bad_arguments. Answers with \
                 the stored device and the previous one. Never takes focus or \
                 switches the visible room or harness. Gated by the Settings switch \
                 that also governs open_harness/close_harness, and rate-capped with \
                 open_design_entry and show_element (10 combined per minute, checked \
                 before validation, so refused attempts count).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "device": {
                        "type": ["object", "null"],
                        "description": "The device setting, or null to clear it.",
                        "properties": {
                            "preset": {
                                "type": "string",
                                "enum": ["none", "custom", "iphone-16-pro", "iphone-14", "android", "ipad-air"],
                            },
                            "width": { "type": "integer", "minimum": 100, "maximum": 4000 },
                            "height": { "type": "integer", "minimum": 100, "maximum": 4000 },
                            "landscape": { "type": "boolean" },
                            "dpr": { "type": "number", "minimum": 0.5, "maximum": 4 },
                            "touch": { "type": "boolean" },
                        },
                        "additionalProperties": false,
                    },
                },
                "required": ["device"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "show_element",
            "title": "Scroll to and highlight an element in the design pane",
            "description":
                "Highlights one element in a design pane of your own room, \
                 transiently — it creates no comment thread and writes nothing. \
                 Give exactly one of `selector` (a CSS selector) or `anchor` (an \
                 element anchor: a `selector` or `tag` string is required; text, \
                 attrs, odId and source are optional). The \
                 answer's `tier` says how it resolved. anchored, reanchored and \
                 selector (a unique match) highlight the element. stale returns the \
                 best candidate in `element` but does NOT highlight it — never a \
                 guess. ambiguous means the selector matched more than one element \
                 (see `count`) and nothing is highlighted. not_found highlights \
                 nothing; when the CSS selector does not parse it also carries \
                 `invalidSelector: true`. `highlighted` says whether anything was. \
                 The pane must be mounted (open in Skein, not necessarily \
                 visible), otherwise this refuses with not_mounted; not_ready means \
                 the preview has not loaded. \
                 Never takes focus or switches the visible room or harness, except \
                 with `reveal: true`, which switches the room's active harness to \
                 this design pane, only when the user is already in this room \
                 (never switches rooms or raises the window). Gated by \
                 the Settings switch that also governs open_harness/close_harness, \
                 and rate-capped with open_design_entry (10 combined per minute, \
                 checked before validation, so refused attempts count).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "selector": {
                        "type": "string",
                        "description": "A CSS selector. Exclusive with anchor.",
                    },
                    "anchor": {
                        "type": "object",
                        "description": "An element anchor. Requires a `selector` or `tag` \
                            string; optional: text, attrs, odId, source. Exclusive with selector.",
                        "properties": {
                            "selector": { "type": "string" },
                            "tag": { "type": "string" },
                            "text": { "type": "string" },
                            "attrs": {},
                            "odId": { "type": "string" },
                            "source": {},
                        },
                    },
                    "reveal": {
                        "type": "boolean",
                        "description": "Switch the room's active harness to this design pane, only when the user is already in this room; never switches rooms or raises the window. Needed for swipes, since a hidden pane has no layout. Default false.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "invoke_element",
            "title": "Tap or swipe an element in the design pane",
            "description":
                "Taps or swipes one element of the page a design pane previews, \
                 as the prototype's own page events (pointer and mouse events, \
                 then a click for a tap). It drives the prototype, not Skein, and \
                 creates no comment thread. Give exactly one of `selector` (a CSS \
                 selector) or `anchor` (same shape as show_element). `action` is \
                 \"tap\" (default) or \"swipe\"; a swipe needs `direction` \
                 (left, right, up, down) and takes an optional `distance` (integer \
                 CSS px 8-2000, default 120); direction and distance are refused \
                 with tap. It acts ONLY on a single, accepted match, never a guess: \
                 selector, anchored and reanchored matches are acted on, while \
                 ambiguous (more than one match, see `count`), stale and not_found \
                 (an unparsable selector also carries `invalidSelector: true`) act \
                 on nothing. `invoked` says whether anything was done; when it \
                 was, `domChanged` says something in the page changed within \
                 300 ms, and false suggests the prototype ignored the event. A \
                 touch-mode device sends touch-type pointer events. A tap fires \
                 pointer and mouse press events at the element's centre and then \
                 element.click(); the click event itself carries no coordinates, \
                 and the tap is delivered to the element even if something covers \
                 it on screen. A swipe starts 4 px inside the edge of the element \
                 you target, so target the element that owns the gesture (e.g. the \
                 screen or frame container whose edge opens a drawer), not an \
                 inner child; prototypes typically measure the edge band against \
                 their own container. Refuses with \
                 busy while the user is picking an element in that pane, \
                 not_mounted when the pane is not open in Skein, and not_ready \
                 while the preview has not loaded, and not_visible for a swipe \
                 while the pane is hidden (no layout; a tap still works). Never \
                 takes focus or switches the visible room or harness, except with \
                 `reveal: true`, which switches the room's active harness to this \
                 design pane, only when the user is already in this room (never \
                 switches rooms or raises the window); needed for swipes. Gated by the Settings switch that \
                 also governs open_harness/close_harness, and rate-capped with \
                 open_design_entry, set_design_device and show_element (10 \
                 combined per minute, checked after argument validation).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "selector": {
                        "type": "string",
                        "description": "A CSS selector. Exclusive with anchor.",
                    },
                    "anchor": {
                        "type": "object",
                        "description": "An element anchor. Requires a `selector` or `tag` \
                            string; optional: text, attrs, odId, source. Exclusive with selector.",
                        "properties": {
                            "selector": { "type": "string" },
                            "tag": { "type": "string" },
                            "text": { "type": "string" },
                            "attrs": {},
                            "odId": { "type": "string" },
                            "source": {},
                        },
                    },
                    "action": {
                        "type": "string",
                        "enum": ["tap", "swipe"],
                        "description": "Default tap.",
                    },
                    "direction": {
                        "type": "string",
                        "enum": ["left", "right", "up", "down"],
                        "description": "Required with swipe; refused with tap.",
                    },
                    "distance": {
                        "type": "integer",
                        "minimum": 8,
                        "maximum": 2000,
                        "description": "Swipe length in CSS px. Default 120. Swipe only.",
                    },
                    "reveal": {
                        "type": "boolean",
                        "description": "Switch the room's active harness to this design pane, only when the user is already in this room; never switches rooms or raises the window. Needed for swipes, since a hidden pane has no layout. Default false.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "show_changes",
            "title": "Outline the elements a diff touches in the design pane",
            "description":
                "Outlines, in the design pane's preview, the rendered elements \
                 whose JSX opening tag (including its attribute lines) is on a \
                 line the chosen diff scope changed, every rendered instance. \
                 `scope` is branch (default), pending or commit (needs \
                 `commit_sha`), as for get_diff. LIMITS, stated plainly: only \
                 changed lines inside a JSX opening tag map to elements. Changes \
                 to logic, styles defined outside the tag, tokens, CSS, plain .js, \
                 closing tags or text, and elements not rendered right now, are \
                 NOT highlighted; they come back in `unmapped` with a reason \
                 (`deleted`, `file_not_rendered`, `no_rendered_tag`). It shows \
                 where changed lines render, not everything that looks different, \
                 so do not read an empty highlight as no visible change. The \
                 answer has `mapped` (path, line, endLine, changedLines, elements, \
                 onScreen), `unmapped`, `highlighted`, `capped` (over 200 \
                 elements), `noSourceInfo` (the page reports no source sites), \
                 `truncated` (more than 300 files or 20000 lines) and `limits`. \
                 A scope with no changes answers without touching the pane. The \
                 outline is transient (Escape or a click clears it) and creates no \
                 thread. Refuses with not_mounted when the pane is not open in \
                 Skein and not_ready while the preview has not loaded. Never takes \
                 focus or switches the visible room or harness, except with \
                 `reveal: true`, which switches the room's active harness to this \
                 design pane, only when the user is already in this room. Gated by \
                 the Settings switch that also governs open_harness/close_harness, \
                 and rate-capped with the other design write verbs (10 combined \
                 per minute, checked after argument validation).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["branch", "pending", "commit"],
                        "description": "Default branch.",
                    },
                    "commit_sha": {
                        "type": "string",
                        "description": "Required with scope commit; refused otherwise.",
                    },
                    "reveal": {
                        "type": "boolean",
                        "description": "Switch the room's active harness to this design pane, only when the user is already in this room; never switches rooms or raises the window. Default false.",
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "get_design_screenshot",
            "title": "Screenshot the design pane's preview",
            "description":
                "Returns a PNG image of what the design pane's preview shows right \
                 now, captured from the app window itself (so the prototype's own \
                 rendering, including its device frame, and not a re-render), plus \
                 JSON text with harnessId, entry, width, height (image px), scale \
                 and cssRect. The pane must be on screen: when it is in another \
                 room, on a hidden tab or not laid out yet this refuses with \
                 not_visible and the reason, and never changes the UI itself; call \
                 show_design_pane first, then retry. `maxEdge` bounds the longest \
                 image edge in px (default 1568, clamped to 256-2576). Refuses \
                 with too_large if the PNG exceeds 4 MiB (lower maxEdge) and \
                 capture_failed if the native capture times out or errors. \
                 not_visible with reason no_pane means the pane is not open in \
                 Skein. Gated by the \
                 Settings switch that also governs open_harness/close_harness, and \
                 rate-capped with the other design verbs (10 combined per minute).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                    "maxEdge": {
                        "type": "integer",
                        "minimum": 256,
                        "maximum": 2576,
                        "description": "Longest edge of the image in px. Default 1568.",
                    },
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "show_design_pane",
            "title": "Make the design pane visible",
            "description":
                "Makes a design harness's pane visible to the user so \
                 get_design_screenshot can see it. This MAY SWITCH THE ACTIVE ROOM \
                 and the visible tab or dock in Skein's window, away from where the \
                 user is looking. That is a deliberate exception to the rule that \
                 design verbs take no focus, and the user's permission for this \
                 tool is the gate: only call it when you need the pane on screen. \
                 It does not raise the window. Answers with shown, switchedRoom \
                 and revealed (already, show_docked, switch or room: what it had \
                 to do), or a reason when it could not. Refuses with not_mounted \
                 when the pane is not open in Skein. Gated by the Settings switch \
                 that also governs open_harness/close_harness, and rate-capped with \
                 the other design verbs (10 combined per minute).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "A design harness id. Optional when the room has exactly one.",
                    },
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": false },
        }),
    ]
}
