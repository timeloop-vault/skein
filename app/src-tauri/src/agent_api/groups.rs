//! Global MCP tool groups (#534): which families of agent-API tools a
//! user lets harnesses see and call.
//!
//! The groups replace the four per-feature `allow_agent_*` kill switches
//! in `SpawnSettings`. Adding a group (say #669's settings verbs, off by
//! default) is one [`ToolGroup`] variant plus one [`ToolGroups`] field
//! with its default; the exhaustive matches below then point at
//! everything else that needs a decision.

use serde::{Deserialize, Serialize};

/// A family of tools that is switched on or off together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolGroup {
    Review,
    Mail,
    Rooms,
    Directory,
    Design,
}

impl ToolGroup {
    pub const ALL: [ToolGroup; 5] = [
        ToolGroup::Review,
        ToolGroup::Mail,
        ToolGroup::Rooms,
        ToolGroup::Directory,
        ToolGroup::Design,
    ];

    /// The stable id, matching the `toolGroups` key in `settings.json`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::Mail => "mail",
            Self::Rooms => "rooms",
            Self::Directory => "directory",
            Self::Design => "design",
        }
    }

    /// Short human label for refusal text.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "Review",
            Self::Mail => "Mail",
            Self::Rooms => "Rooms",
            Self::Directory => "Directory",
            Self::Design => "Design",
        }
    }
}

/// The group a bare tool name belongs to. `None` means always available
/// (`skein_info`) or not a tool at all.
#[must_use]
pub fn group_of(tool: &str) -> Option<ToolGroup> {
    match tool {
        "list_comments" | "get_comment" | "get_diff" | "reply" | "mark_addressed"
        | "review_status" => Some(ToolGroup::Review),
        "send_message" | "read_messages" | "message_history" => Some(ToolGroup::Mail),
        "create_room" | "close_room" | "open_harness" | "close_harness" => Some(ToolGroup::Rooms),
        "find_rooms_for_path" | "list_rooms" | "get_room" | "list_harnesses" => {
            Some(ToolGroup::Directory)
        }
        "list_design_harnesses"
        | "get_design_state"
        | "open_design_entry"
        | "set_design_device"
        | "show_element"
        | "invoke_element"
        | "show_changes"
        | "get_design_screenshot"
        | "show_design_pane" => Some(ToolGroup::Design),
        _ => None,
    }
}

/// Per-group on/off switches, persisted as `toolGroups` in `settings.json`.
/// Every field defaults on, so a missing key never silently disables one.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolGroups {
    pub review: bool,
    pub mail: bool,
    pub rooms: bool,
    pub directory: bool,
    pub design: bool,
}

impl Default for ToolGroups {
    fn default() -> Self {
        Self {
            review: true,
            mail: true,
            rooms: true,
            directory: true,
            design: true,
        }
    }
}

impl ToolGroups {
    #[must_use]
    pub fn allows(self, g: ToolGroup) -> bool {
        match g {
            ToolGroup::Review => self.review,
            ToolGroup::Mail => self.mail,
            ToolGroup::Rooms => self.rooms,
            ToolGroup::Directory => self.directory,
            ToolGroup::Design => self.design,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_tool_is_grouped_or_skein_info() {
        for spec in crate::agent_api::mcp::tool_specs() {
            let name = spec["name"].as_str().expect("tool has a name");
            assert!(
                name == "skein_info" || group_of(name).is_some(),
                "tool `{name}` is in no ToolGroup; add it to group_of"
            );
        }
    }

    #[test]
    fn keys_are_distinct_and_allows_matches_fields() {
        let off = ToolGroups {
            mail: false,
            ..ToolGroups::default()
        };
        for g in ToolGroup::ALL {
            assert_eq!(off.allows(g), g != ToolGroup::Mail, "{}", g.key());
        }
    }
}
