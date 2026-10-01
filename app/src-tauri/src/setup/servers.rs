//! The two localhost listeners bound during setup.

use std::sync::Arc;

use tauri::Manager;

use crate::db::Database;

/// The agent-facing review API (#213).
pub(super) fn agent_api(app: &mut tauri::App, db: &Arc<Database>) {
    // Bound with the std
    // listener so a failure is known *here*, synchronously, and
    // can be recorded — an async bind inside the spawned task
    // would fail into a log line nobody reads and leave the
    // settings pane claiming a port that never existed.
    //
    // 127.0.0.1 only, and an ephemeral port: the URL reaches
    // the harnesses through SKEIN_REVIEW_URL at spawn time, so
    // nothing has to agree on a number in advance.
    let endpoint = match std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| {
        let port = l.local_addr()?.port();
        l.set_nonblocking(true)?;
        Ok((l, port))
    }) {
        Ok((listener, port)) => {
            let state = Arc::new(crate::agent_api::AgentApiState::new(
                Arc::clone(db),
                app.handle().clone(),
            ));
            // Managed so `agent_request_complete` (#328) can
            // reach the same pending-request map the HTTP
            // server's handlers register requests on.
            app.manage(Arc::clone(&state));
            tauri::async_runtime::spawn(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => crate::agent_api::http::serve(listener, state).await,
                    Err(e) => {
                        tracing::error!(error = %e, "agent api: adopting listener failed");
                    }
                }
            });
            tracing::info!(port, "agent api listening on 127.0.0.1");
            crate::agent_api::state::AgentApiEndpoint::bound(port)
        }
        Err(e) => {
            // Not fatal: Skein is perfectly usable without the
            // agent API. But it must be *visible* — an agent
            // whose tools silently do not exist is the worst of
            // both worlds (#176).
            tracing::error!(error = %e, "agent api: bind failed; agents cannot reach the review");
            crate::agent_api::state::AgentApiEndpoint::failed(e.to_string())
        }
    };
    app.manage(endpoint);
}

/// The design preview server (#433). A separate listener
/// from the agent API, and not fatal when the bind
/// fails; see the `design` module doc for why.
pub(super) fn design_preview(app: &mut tauri::App, db: &Arc<Database>) {
    let preview_state = Arc::new(crate::design::PreviewState::new(Arc::clone(db)));
    let preview_endpoint = match std::net::TcpListener::bind(("127.0.0.1", 0)).and_then(|l| {
        let port = l.local_addr()?.port();
        l.set_nonblocking(true)?;
        Ok((l, port))
    }) {
        Ok((listener, port)) => {
            let state = Arc::clone(&preview_state);
            tauri::async_runtime::spawn(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => crate::design::serve(listener, state).await,
                    Err(e) => {
                        tracing::error!(error = %e, "design preview: adopting listener failed");
                    }
                }
            });
            tracing::info!(port, "design preview listening on 127.0.0.1");
            crate::design::commands::PreviewEndpoint::bound(port)
        }
        Err(e) => {
            tracing::error!(error = %e, "design preview: bind failed");
            crate::design::commands::PreviewEndpoint::failed(e.to_string())
        }
    };
    app.manage(preview_state);
    app.manage(preview_endpoint);
}
