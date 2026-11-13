//! What boot preparation writes (#755): a per-harness plan applied field by
//! field under the store lock, never a whole-harness put. The plan is built
//! outside the lock from the snapshot the load returned; the rule that
//! decides each harness runs inside `RoomStore::edit_harnesses`, on the
//! record as stored at that moment.

use std::collections::HashMap;

use skein_proto::rooms::{Harness, LaunchMode, Room, ShellClaim};

use crate::harness_argv::migrate_launch;

/// The fields the plan was computed from. If the stored record differs in any
/// of them, something wrote it since the snapshot and the plan is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Expect {
    pub launch: Option<LaunchMode>,
    pub shell_claim: Option<ShellClaim>,
    pub cmd: Option<Vec<String>>,
    pub session_id: Option<String>,
}

/// The only fields boot preparation may write. Never `session_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Set {
    pub launch: Option<LaunchMode>,
    pub shell_claim: Option<ShellClaim>,
    pub cmd: Option<Vec<String>>,
}

/// One harness's entry in the plan.
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessPlan {
    pub expect: Expect,
    pub set: Set,
    /// What `migrate_launch` alone gives this harness: the one step that runs
    /// for a harness with a process too, and the only write a busy one takes.
    pub migrated_launch: Option<LaunchMode>,
}

/// What the rule did to one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The three `set` fields were written.
    Full,
    /// The stored record no longer matches `expect`; only the launch
    /// migration (when `launch`, `cmd` and `shell_claim` were all still
    /// unchanged) was written.
    Stale,
    /// The harness has (or is getting) a process; only `launch` was migrated.
    Busy,
}

/// How many harnesses went each way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    pub applied: usize,
    pub stale: usize,
    pub busy: usize,
}

impl Counts {
    pub fn record(&mut self, a: Applied) {
        match a {
            Applied::Full => self.applied += 1,
            Applied::Stale => self.stale += 1,
            Applied::Busy => self.busy += 1,
        }
    }
}

impl HarnessPlan {
    /// The apply rule, on the record as stored now, in this order. Writes
    /// nothing but `launch`, `shell_claim` and `cmd`.
    ///
    /// 1. The launch migration does not wait for the session id or for the
    ///    harness to be idle: if the stored `launch`, `cmd` and `shell_claim`
    ///    (what the migration was inferred from) still equal `expect`,
    ///    `launch` becomes `migrated_launch`, for busy harnesses and for ones
    ///    whose `session_id` changed too. A record whose `cmd` a client
    ///    rewrote meanwhile (a restart back to the tool) is left alone.
    /// 2. If `shell_claim`, `cmd`, `launch` or `session_id` differs from
    ///    `expect`: `Stale`, nothing more is written.
    /// 3. Else if busy: `Busy`, nothing more.
    /// 4. Else the three `set` fields are written: `Full`.
    pub fn apply(&self, h: &mut Harness, busy: bool) -> Applied {
        let e = &self.expect;
        let inputs_unchanged =
            h.launch == e.launch && h.shell_claim == e.shell_claim && h.cmd == e.cmd;
        if inputs_unchanged {
            h.launch.clone_from(&self.migrated_launch);
        }
        if !inputs_unchanged || h.session_id != e.session_id {
            return Applied::Stale;
        }
        if busy {
            return Applied::Busy;
        }
        h.launch.clone_from(&self.set.launch);
        h.shell_claim.clone_from(&self.set.shell_claim);
        h.cmd.clone_from(&self.set.cmd);
        Applied::Full
    }
}

/// The harnesses that differ between `before` and `after` (same rooms, same
/// harness order: preparation never adds or removes either), keyed by id.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct BootPlan(HashMap<String, HarnessPlan>);

impl BootPlan {
    #[must_use]
    pub fn new(before: &[Room], after: &[Room]) -> Self {
        let same =
            |a: &Harness, b: &Harness| serde_json::to_value(a).ok() == serde_json::to_value(b).ok();
        let mut plan = HashMap::new();
        for (b, a) in before.iter().zip(after) {
            let mut migrated: Option<Room> = None;
            for (hb, ha) in b.harnesses.iter().zip(&a.harnesses) {
                if same(hb, ha) {
                    continue;
                }
                let migrated = migrated.get_or_insert_with(|| {
                    let mut r = b.clone();
                    migrate_launch(&mut r);
                    r
                });
                let migrated_launch = migrated
                    .harnesses
                    .iter()
                    .find(|m| m.id == hb.id)
                    .and_then(|m| m.launch.clone());
                plan.insert(
                    hb.id.clone(),
                    HarnessPlan {
                        expect: Expect {
                            launch: hb.launch.clone(),
                            shell_claim: hb.shell_claim.clone(),
                            cmd: hb.cmd.clone(),
                            session_id: hb.session_id.clone(),
                        },
                        set: Set {
                            launch: ha.launch.clone(),
                            shell_claim: ha.shell_claim.clone(),
                            cmd: ha.cmd.clone(),
                        },
                        migrated_launch,
                    },
                );
            }
        }
        Self(plan)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, harness_id: &str) -> Option<&HarnessPlan> {
        self.0.get(harness_id)
    }
}
