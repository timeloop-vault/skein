//! The delivery driver (#733): one worker thread, `skein-delivery`, owns the
//! pure [`DeliveryCore`] and its [`DeliveryEnv`], and loops on a channel with
//! a timeout set by the core's next deadline. Everything else talks to it
//! through a cloneable [`DeliveryHandle`].
//!
//! Three ways in, none of which can deadlock against the worker:
//!
//! - [`DeliveryHandle::notify`]: a fire-and-forget channel send. The phase
//!   listener (which runs under the phase service's lock) uses only this.
//! - the blocking calls ([`send_manual`](DeliveryHandle::send_manual),
//!   [`deliver_now`](DeliveryHandle::deliver_now)): a send plus a wait with
//!   a timeout, for client threads that hold no daemon lock.
//! - the shared facts ([`note_output`](DeliveryHandle::note_output),
//!   [`report_client_facts`](DeliveryHandle::report_client_facts)): one short
//!   mutex in [`Shared`], never the channel (the PTY coalescer must not queue
//!   behind the worker).
//!
//! The worker ends when the last handle is dropped.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::time::{Duration, Instant};

use super::core::{DeliveryCore, DeliveryEvent, Reply};
use super::env::{DeliveryEnv, DraftState, MailFacts};
use super::gate::{GateMode, Refusal, RefusalCode};
use super::shared::Shared;
use crate::harness_phase::PhaseNotice;
use crate::harness_phase::service::Clock;

/// How long a blocking call waits for the worker.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

enum Msg {
    Event(DeliveryEvent),
    /// A blocking call; the worker skips it once `deadline` has passed (the
    /// caller has already told its user it failed).
    Call(DeliveryEvent, SyncSender<Reply>, Instant),
}

/// The cloneable front of the worker. See the module docs.
#[derive(Clone)]
pub struct DeliveryHandle {
    tx: Sender<Msg>,
    shared: Arc<Shared>,
}

/// What a phase listener's notice means to the delivery core.
#[must_use]
pub fn event_for_notice(n: PhaseNotice) -> DeliveryEvent {
    match n {
        PhaseNotice::Spawned { harness_id } => DeliveryEvent::Spawned { harness_id },
        PhaseNotice::Transition {
            harness_id,
            from,
            to,
        } => DeliveryEvent::Transition {
            harness_id,
            from,
            to,
        },
        PhaseNotice::DeferralArmed { harness_id } => DeliveryEvent::DeferralArmed { harness_id },
        PhaseNotice::Forgotten { harness_id } => DeliveryEvent::Forgotten { harness_id },
    }
}

impl DeliveryHandle {
    /// Fire and forget. A `Spawned` or `Forgotten` first drops the harness's
    /// stored output state and client reports, on the caller's thread, so the
    /// new process never sees the old one's facts.
    pub fn notify(&self, ev: DeliveryEvent) {
        if let DeliveryEvent::Spawned { harness_id } | DeliveryEvent::Forgotten { harness_id } = &ev
        {
            self.shared.reset(harness_id);
        }
        if self.tx.send(Msg::Event(ev)).is_err() {
            tracing::debug!("prompt delivery: worker is gone; event dropped");
        }
    }

    /// The listener to register on the phase service.
    #[must_use]
    pub fn phase_listener(&self) -> crate::harness_phase::PhaseListener {
        let handle = self.clone();
        Arc::new(move |n| handle.notify(event_for_notice(n)))
    }

    /// A review nudge or prompt-library item: pasted and submitted if the
    /// gate allows, with no draft check, no hold and no retry. Blocks until
    /// the paste is written (not until the submit).
    ///
    /// # Errors
    /// The gate's refusal, or `Busy` / `NotReady` when the worker does not
    /// answer in time or is gone.
    pub fn send_manual(&self, harness_id: &str, body: &str) -> Result<(), Refusal> {
        match self.call(
            DeliveryEvent::Manual {
                harness_id: harness_id.to_owned(),
                body: body.to_owned(),
            },
            CALL_TIMEOUT,
        )? {
            Reply::Manual(r) => r,
            other => Err(unexpected(&other)),
        }
    }

    /// Read-only: what the gate would answer for this harness right now
    /// (`Send`: what [`send_manual`](Self::send_manual) would, including
    /// `Busy` while a prompt is in flight; `Insert`: the file-drop gate).
    /// Writes nothing and records nothing. `body: None` is an empty
    /// single-line body. Runs on the worker, which owns the in-flight state.
    ///
    /// # Errors
    /// The gate's refusal, or `Busy` / `NotReady` when the worker does not
    /// answer in time or is gone.
    pub fn check(
        &self,
        harness_id: &str,
        body: Option<&str>,
        mode: GateMode,
    ) -> Result<(), Refusal> {
        match self.call(
            DeliveryEvent::Check {
                harness_id: harness_id.to_owned(),
                body: body.map(str::to_owned),
                mode,
            },
            CALL_TIMEOUT,
        )? {
            Reply::Check(r) => r,
            other => Err(unexpected(&other)),
        }
    }

    /// #413: the user released held mail.
    ///
    /// # Errors
    /// As [`send_manual`](Self::send_manual), plus `NothingToDeliver`.
    pub fn deliver_now(&self, harness_id: &str) -> Result<(), Refusal> {
        self.deliver_now_within(harness_id, CALL_TIMEOUT)
    }

    pub(super) fn deliver_now_within(
        &self,
        harness_id: &str,
        timeout: Duration,
    ) -> Result<(), Refusal> {
        match self.call(
            DeliveryEvent::DeliverNow {
                harness_id: harness_id.to_owned(),
            },
            timeout,
        )? {
            Reply::DeliverNow(r) => r,
            other => Err(unexpected(&other)),
        }
    }

    /// The client's latest composer draft and user-input count (#383).
    /// Always stored (unless `report`, the client's per-harness rising
    /// number, is not newer than the stored one); the worker is asked to
    /// re-check only when the draft went Held -> Clean or this is the first
    /// report since the spawn, so a keystroke does not run a mail check.
    pub fn report_client_facts(
        &self,
        harness_id: &str,
        draft: DraftState,
        user_input_seq: u64,
        report: u64,
    ) {
        if self
            .shared
            .set_client(harness_id, draft, user_input_seq, report)
        {
            self.notify(DeliveryEvent::ClientFactsChanged {
                harness_id: harness_id.to_owned(),
            });
        }
    }

    /// A chunk of the harness's PTY output, from the PTY's coalescer thread.
    pub fn note_output(&self, harness_id: &str, chunk: &str, now_ms: i64) {
        self.shared.note_output(harness_id, chunk, now_ms);
    }

    /// Harness ids whose mail is currently held by the draft guard.
    #[must_use]
    pub fn held_snapshot(&self) -> Vec<String> {
        self.shared.held_snapshot()
    }

    /// The mail facts the supervisor reads (#733 part B): the last nudge's
    /// time and the refusal holding pending mail. A shared read, never a
    /// worker round trip.
    #[must_use]
    pub fn supervisor_mail(&self, harness_id: &str) -> MailFacts {
        self.shared.mail_facts(harness_id)
    }

    /// The shared facts, for the real env to be built over.
    #[must_use]
    pub fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    fn call(&self, ev: DeliveryEvent, timeout: Duration) -> Result<Reply, Refusal> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.tx
            .send(Msg::Call(ev, reply_tx, Instant::now() + timeout))
            .map_err(|_| gone("the delivery worker is not running"))?;
        reply_rx.recv_timeout(timeout).map_err(|e| match e {
            RecvTimeoutError::Timeout => Refusal {
                code: RefusalCode::Busy,
                reason: "the delivery worker did not answer in time".to_owned(),
            },
            RecvTimeoutError::Disconnected => gone("the delivery worker stopped"),
        })
    }
}

fn gone(reason: &str) -> Refusal {
    Refusal {
        code: RefusalCode::NotReady,
        reason: reason.to_owned(),
    }
}

fn unexpected(reply: &Reply) -> Refusal {
    tracing::error!(?reply, "prompt delivery: unexpected reply kind");
    gone("the delivery worker gave an unexpected answer")
}

/// Starts the worker.
pub struct DeliveryService;

impl DeliveryService {
    /// Spawn the `skein-delivery` thread over `env`, reading `clock` (the
    /// phase service's, so deadlines and the phase records share one time).
    /// `shared` must be the one `env` reads. If the OS refuses the thread
    /// the handle is still returned (logged): its events are then dropped and
    /// its blocking calls refuse.
    pub fn start(
        env: Box<dyn DeliveryEnv + Send>,
        shared: Arc<Shared>,
        clock: Clock,
    ) -> DeliveryHandle {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("skein-delivery".to_owned())
            .spawn(move || run(env, &rx, &clock));
        if let Err(e) = spawned {
            tracing::error!(error = %e, "prompt delivery: could not start the worker thread");
        }
        DeliveryHandle { tx, shared }
    }
}

/// Milliseconds to wait until `deadline`, at least 0.
fn wait_until(deadline: i64, now: i64) -> Duration {
    Duration::from_millis(u64::try_from(deadline.saturating_sub(now)).unwrap_or(0))
}

fn run(mut env: Box<dyn DeliveryEnv + Send>, rx: &Receiver<Msg>, clock: &Clock) {
    let mut core = DeliveryCore::new();
    loop {
        let received = match core.next_deadline() {
            Some(deadline) => rx.recv_timeout(wait_until(deadline, clock())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(msg) => step(&mut core, env.as_mut(), clock, msg),
            Err(RecvTimeoutError::Timeout) => {
                guarded("timer", || core.on_timer(env.as_mut(), clock()));
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn step(core: &mut DeliveryCore, env: &mut dyn DeliveryEnv, clock: &Clock, msg: Msg) {
    match msg {
        Msg::Event(ev) => {
            guarded("event", || {
                core.handle(env, ev, clock());
            });
        }
        Msg::Call(ev, reply, deadline) => {
            if Instant::now() >= deadline {
                tracing::warn!("prompt delivery: a call expired before the worker reached it");
                return;
            }
            // A panic drops `reply`: the caller sees the worker "stopped".
            if let Some(r) = guarded("call", || core.handle(env, ev, clock())) {
                let _ = reply.send(r);
            }
        }
    }
}

/// A panic in the core or the env must not end the worker: every later
/// event would go nowhere.
fn guarded<R>(what: &str, f: impl FnOnce() -> R) -> Option<R> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    if r.is_err() {
        tracing::error!(what, "prompt delivery: worker step panicked; continuing");
    }
    r.ok()
}
