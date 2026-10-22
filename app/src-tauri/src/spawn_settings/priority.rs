//! The "what priority does a harness child start at" decision, kept
//! pure so the spawn wiring only has to act on the answer.

use super::SpawnSettings;

/// CPU priority a harness PTY child is spawned at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildPriority {
    /// Whatever the OS gives a child of Skein.
    Normal,
    /// Below normal (Windows) or nice +10 (macOS/Linux).
    Lowered,
}

impl SpawnSettings {
    /// The priority a harness spawned under these settings should get.
    pub(crate) fn child_priority(&self) -> ChildPriority {
        if self.lower_harness_priority {
            ChildPriority::Lowered
        } else {
            ChildPriority::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_priority_follows_the_setting() {
        let on = SpawnSettings {
            lower_harness_priority: true,
            ..SpawnSettings::default()
        };
        assert_eq!(on.child_priority(), ChildPriority::Lowered);
        let off = SpawnSettings {
            lower_harness_priority: false,
            ..SpawnSettings::default()
        };
        assert_eq!(off.child_priority(), ChildPriority::Normal);
    }
}
