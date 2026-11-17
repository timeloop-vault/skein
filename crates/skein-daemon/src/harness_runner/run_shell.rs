//! `run_shell` (#756): "press Enter for shell" asked of the daemon in one call.

use skein_proto::rooms::LaunchMode;

use super::gates::{SpawnPlan, lookup};
use super::{EnsureError, EnsureRequest, HarnessRunner, RUNNER_ORIGIN, Refusal};

impl HarnessRunner {
    /// Switch the harness to the post-exit shell: `launch = Shell { argv }`
    /// and no `shell_claim`, in one field-level edit. The caller holds the
    /// harness's spawn gate; the store is called with no runner lock held
    /// besides it (the documented lock order).
    ///
    /// Every refusal `ensure_running` can produce before the spawn is decided
    /// first, so a refused request writes nothing: the empty argv, the lookup
    /// refusals, a stale `expect_cmd`, and the spawn plan's own (`no_cmd`,
    /// `folder_missing`, `repo_mismatch`), which is built here as a dry run
    /// over a copy of the record as it will be after the switch. Only then is
    /// the edit written. What can still fail afterwards is not a refusal: the
    /// OS failing to start the process (`spawn_failed`), or the records
    /// changing under another client between the edit and the spawn; both
    /// leave the record in shell mode, as a failed tool spawn leaves its
    /// record.
    pub(super) fn switch_to_shell(
        &self,
        req: &EnsureRequest,
        argv: &[String],
    ) -> Result<(), EnsureError> {
        if argv.is_empty() {
            return Err(EnsureError::Refused(Refusal::EmptyShellArgv));
        }
        let rooms = self.store.rooms();
        let (room, harness, kind) =
            lookup(&rooms, &req.room_id, &req.harness_id).map_err(EnsureError::Refused)?;
        if let Some(expect) = &req.expect_cmd
            && harness.cmd.as_ref() != Some(expect)
        {
            return Err(EnsureError::Refused(Refusal::StaleRecord));
        }
        let launch = LaunchMode::Shell {
            argv: argv.to_vec(),
        };
        // Dry run. A shell launch derives its argv from the record alone, so
        // neither the session probe nor the port pick is reached.
        let mut after = harness.clone();
        after.launch = Some(launch.clone());
        after.shell_claim = None;
        SpawnPlan::from_record(
            room,
            &after,
            kind,
            req.env_profile.as_deref(),
            &|_| Ok(true),
            &|| Err("not picked in a dry run".to_owned()),
        )
        .map_err(EnsureError::Refused)?;
        self.store
            .edit_harness(RUNNER_ORIGIN, &req.room_id, &req.harness_id, |h| {
                h.launch = Some(launch);
                h.shell_claim = None;
            })
            .map_err(|e| EnsureError::SpawnFailed(format!("recording the shell launch: {e}")))?;
        Ok(())
    }
}
