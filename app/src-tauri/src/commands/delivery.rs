//! The webview's side of the daemon's prompt delivery (#733): a manual
//! send, "deliver held mail now", the client-only facts the gate needs
//! (composer draft, keystroke counter) and the held-mail snapshot.

use skein_daemon::prompt_delivery::{DeliveryHandle, DraftState, GateMode, Refusal, RefusalCode};
use tauri::Manager;

/// The outcome of a send, as the webview reads it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<RefusalCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl From<Result<(), Refusal>> for SendResult {
    fn from(r: Result<(), Refusal>) -> Self {
        match r {
            Ok(()) => Self {
                ok: true,
                code: None,
                reason: None,
            },
            Err(e) => Self {
                ok: false,
                code: Some(e.code),
                reason: Some(e.reason),
            },
        }
    }
}

/// What the composer holds, as the client reports it.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct ClientDraft {
    held: bool,
    #[serde(default)]
    reason: Option<String>,
}

impl From<ClientDraft> for DraftState {
    fn from(d: ClientDraft) -> Self {
        if d.held {
            Self::Held {
                reason: d
                    .reason
                    .unwrap_or_else(|| "the composer holds a draft".to_owned()),
            }
        } else {
            Self::Clean
        }
    }
}

/// The handle's calls wait on the delivery worker, so they run on the
/// blocking pool, never on a runtime thread.
async fn blocking<T: Send + 'static>(
    app: &tauri::AppHandle,
    f: impl FnOnce(DeliveryHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let handle = (*app.state::<DeliveryHandle>()).clone();
    tauri::async_runtime::spawn_blocking(move || f(handle))
        .await
        .map_err(|e| e.to_string())
}

/// Sends a prompt the user composed into the harness's PTY.
#[tauri::command]
pub(crate) async fn harness_send_prompt(
    harness_id: String,
    body: String,
    app: tauri::AppHandle,
) -> Result<SendResult, String> {
    blocking(&app, move |d| d.send_manual(&harness_id, &body).into()).await
}

/// Read-only: would the send gate (`send`) or the file-drop gate (`insert`)
/// allow this now? Never writes to the PTY or changes delivery state.
#[tauri::command]
pub(crate) async fn harness_gate_check(
    harness_id: String,
    body: Option<String>,
    mode: GateMode,
    app: tauri::AppHandle,
) -> Result<SendResult, String> {
    blocking(&app, move |d| {
        d.check(&harness_id, body.as_deref(), mode).into()
    })
    .await
}

/// Delivers the harness's held mail now, past the draft guard.
#[tauri::command]
pub(crate) async fn mail_deliver_now(
    harness_id: String,
    app: tauri::AppHandle,
) -> Result<SendResult, String> {
    blocking(&app, move |d| d.deliver_now(&harness_id).into()).await
}

/// The composer draft and keystroke counter only the client can see.
/// `report` is the webview's own per-harness rising number: invokes can be
/// handled out of order and the daemon keeps only the newest. A plain
/// command: it takes one mutex and sends on a channel, never waits.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn harness_client_facts(
    harness_id: String,
    draft: ClientDraft,
    user_input_seq: u64,
    report: u64,
    delivery: tauri::State<'_, DeliveryHandle>,
) {
    delivery.report_client_facts(&harness_id, draft.into(), user_input_seq, report);
}

/// Harness ids whose mail is currently held by the draft guard.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn mail_held_snapshot(delivery: tauri::State<'_, DeliveryHandle>) -> Vec<String> {
    delivery.held_snapshot()
}
