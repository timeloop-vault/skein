//! The MCP-level `instructions` text a client sees at `initialize`,
//! assembled from the tool groups that are on (#534). A group that is
//! off contributes nothing: an agent with review off is never told about
//! `review_status` or the sign-off.

use super::super::groups::{ToolGroup, ToolGroups};

const INTRO: &str = "This server is Skein's agent API for the room this token belongs to.";

const REVIEW: &str = "Review: use list_comments to see the reviewer's outstanding \
     comments, get_comment to read one together with the code it is \
     about, and get_diff to read the change under review. Answer with \
     reply, and use mark_addressed once you have made the change (give \
     the commit sha when you have one). You cannot resolve threads: the \
     reviewer closes them after reading your reply. Before you merge, \
     push, or open a pull request, call review_status — it says whether \
     the reviewer has signed off on the commit you are about to land. \
     You cannot sign off yourself.";

const MAIL: &str = "Mail: send_message sends a short message to another harness — give \
     `to` as a harness id or a room id, which is routed to that room's \
     lead harness. read_messages returns your unread messages oldest \
     first and marks them read; pass include_read for the whole \
     history. When Skein tells you that you have new messages, call \
     read_messages. message_history is the read-only counterpart: \
     filtered by counterpart, paged with since/limit, and — with \
     direction — able to show what you sent as well, all without \
     marking anything read; reach for it to rebuild a thread after \
     your own context is compacted.";

const ROOMS: &str = "Rooms: create_room opens a real Skein room and spawns a real agent \
     in it, in the background — not a simulation. Give it a prompt and \
     that agent starts working unattended the moment it exists. \
     close_room archives a room you created, once its reviewer sign-off is \
     approved for its current HEAD — that is the only way to close one: \
     you cannot close your own room, and you cannot otherwise destroy \
     one at all. Archiving keeps the worktree, the branch and the room \
     record; the user can reopen it any time. open_harness adds a \
     harness to your own room, or to one you opened, in the background — \
     never taking focus. close_harness stops one the way closing its tab \
     does — your own room's harnesses (never itself), or any harness in \
     a room you opened — refusing on the room's last harness (use \
     close_room instead), an open permission dialog, or unsaved Files \
     buffers; closing mid-turn is allowed, and the reply names the phase \
     it interrupted.";

const DIRECTORY: &str = "Directory: find_rooms_for_path lists every room, across all projects, whose \
     folder is, contains, or sits under a path; a room with \
     safe_to_remove false is open, so do not touch its folder. \
     list_rooms (optionally created_by: \"me\" for only the rooms you \
     opened), get_room and list_harnesses read rooms and harnesses \
     across the whole install, including ones you did not create — use \
     them to rebuild a picture of a room after your own context is \
     compacted, without replaying history. A harness's phase may read \
     \"unknown\" when Skein cannot currently vouch for it.";

const DESIGN: &str = "Design pane: list_design_harnesses, get_design_state, \
     open_design_entry, set_design_device, show_element and invoke_element read and drive the design \
     harnesses of your own room only, and never take focus or switch \
     the visible room or harness. show_element highlights an element in \
     the pane transiently and creates no thread. invoke_element taps or \
     swipes an element of the prototype itself, as page events. show_changes \
     outlines the rendered elements whose JSX opening tag is on a line a \
     diff scope changed, and lists everything it cannot map as unmapped. \
     get_design_screenshot returns an image of the preview and refuses \
     (not_visible) when the pane is not on screen; show_design_pane makes \
     it visible and, unlike the rest, may switch the active room and tab.";

fn paragraph(g: ToolGroup) -> &'static str {
    match g {
        ToolGroup::Review => REVIEW,
        ToolGroup::Mail => MAIL,
        ToolGroup::Rooms => ROOMS,
        ToolGroup::Directory => DIRECTORY,
        ToolGroup::Design => DESIGN,
    }
}

/// The instructions for the groups that are on.
pub(super) fn instructions(groups: ToolGroups) -> String {
    let mut out = String::from(INTRO);
    for g in ToolGroup::ALL.into_iter().filter(|g| groups.allows(*g)) {
        out.push_str("\n\n");
        out.push_str(paragraph(g));
    }
    out
}
