//! The spawn-environment report types.

use serde::Serialize;

/// Why one of the user's `PATH` additions didn't make it in.
///
/// Reported rather than silently swallowed: "I added a directory and
/// nothing happened" is the exact confusion this whole feature exists
/// to end, and a dropped entry with no explanation reproduces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// References a variable that isn't set, or a `~user` form.
    Unresolved,
    /// Relative. A relative entry — `.` above all — makes an agent run
    /// whatever happens to be in the working directory.
    NotAbsolute,
    /// Contains a character that can't appear in a single `PATH` entry.
    Separator,
    /// The directory doesn't exist.
    Missing,
    /// Already on the `PATH`.
    Duplicate,
}
