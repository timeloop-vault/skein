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
                 Never takes focus or switches the visible room or harness. Gated by \
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
                },
                "additionalProperties": false,
            },
        }),
    ]
}
