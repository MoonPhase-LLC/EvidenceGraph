//! S1-04: launches, supervises, and authenticates against the local
//! FastAPI analysis service. See `supervisor` module docs for the full
//! startup/shutdown contract. The frontend never receives the session
//! token or the raw HTTP connection -- it only ever sees a
//! [`supervisor::SupervisorState`] snapshot and the result of
//! [`check_service_health`], both narrowly-scoped `#[tauri::command]`s
//! (see that function's docs for why this is the smaller exposed
//! capability versus letting the WebView call the service directly).
//!
//! `supervisor::start`/`shutdown` touch only in-memory state
//! (`SupervisorHandle`), with no Tauri event emission built in -- kept
//! that way deliberately so the core startup/shutdown logic has no Tauri
//! dependency and is directly unit-testable (see `supervisor`'s test
//! suite) without a mock `AppHandle`. The frontend observes progress by
//! polling `get_service_status` on a short interval while not yet
//! `Ready`/`Failed`.

mod supervisor;

use supervisor::{SupervisorHandle, SupervisorState};
use tauri::Manager;

/// Read-only snapshot of the current supervisor state -- safe to call at
/// any time, including before the service is `Ready`. The frontend polls
/// this on a short interval until it observes `Ready` or `Failed`.
#[tauri::command]
async fn get_service_status(
    state: tauri::State<'_, SupervisorHandle>,
) -> Result<SupervisorState, ()> {
    Ok(state.snapshot().await)
}

/// The one narrowly-scoped command that actually talks to the local
/// service, on the frontend's behalf, entirely in Rust: performs a single
/// authenticated `GET /health` request using the connection/token
/// [`SupervisorHandle::ready_connection`] holds, and returns the parsed
/// JSON body or a sanitized error string. This satisfies S1-04's
/// "frontend-to-service authenticated health round trip" requirement
/// without ever handing the session token, host, or port to the WebView's
/// JavaScript context -- the smaller exposed capability versus the
/// alternative (giving the frontend the raw connection details and
/// loosening CSP `connect-src` to let it call the service directly),
/// which is also why no CORS configuration is needed on the FastAPI side
/// for this at all (a same-process Rust HTTP client never sends an
/// `Origin` header, so CORS is not the relevant boundary here -- the
/// session-token check is). Returns `Err("service_not_ready")` -- never a
/// stale/partial connection -- if called before the state is `Ready`.
#[tauri::command]
async fn check_service_health(
    state: tauri::State<'_, SupervisorHandle>,
) -> Result<serde_json::Value, String> {
    let connection = state
        .ready_connection()
        .await
        .ok_or_else(|| "service_not_ready".to_string())?;
    let url = format!("http://{}:{}/health", connection.host, connection.port);
    let response = connection
        .http_client
        .get(&url)
        .bearer_auth(&connection.token)
        .send()
        .await
        .map_err(|_| "health_request_failed".to_string())?;

    if !response.status().is_success() {
        return Err(format!(
            "health_request_unexpected_status_{}",
            response.status().as_u16()
        ));
    }

    response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "health_response_not_json".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let supervisor_handle = SupervisorHandle::new();

    let app = tauri::Builder::default()
        .manage(supervisor_handle.clone())
        .invoke_handler(tauri::generate_handler![
            get_service_status,
            check_service_health
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            let handle = supervisor_handle.clone();
            // Startup runs on Tauri's own async runtime, concurrently with
            // the window opening -- the placeholder screen renders
            // immediately and shows "Starting" (or whatever
            // `get_service_status` returns) rather than blocking window
            // creation on the child process/handshake.
            tauri::async_runtime::spawn(async move {
                supervisor::start(app_handle, handle).await;
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            // Prevent the default immediate exit so the graceful
            // shutdown sequence (private-channel `shutdown` message,
            // bounded grace period, then a forced kill if needed) can run
            // to completion first -- "stop accepting frontend access
            // immediately ... wait for a bounded grace period ...
            // forcefully terminate ... if graceful shutdown fails."
            api.prevent_exit();
            let handle = app_handle.state::<SupervisorHandle>().inner().clone();
            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                supervisor::shutdown(&handle).await;
                app_handle.exit(0);
            });
        }
    });
}
