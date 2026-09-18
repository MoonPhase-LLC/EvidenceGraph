//! S1-04: launches, supervises, and authenticates against the local
//! FastAPI analysis service. See `supervisor` module docs for the full
//! startup/shutdown contract. The frontend never receives the session
//! token or the raw HTTP connection -- it only ever sees a
//! [`supervisor::SupervisorState`] snapshot and the result of
//! [`check_service_health`], both narrowly-scoped `#[tauri::command]`s.
//! `check_service_health` is a thin wrapper over
//! [`supervisor::check_service_health`] (see that function's docs for why
//! this design -- a Rust-side request, never a WebView `fetch` -- was
//! chosen, and for the child-death/stale-connection race it specifically
//! guards against).
//!
//! `supervisor::start`/`shutdown` touch only in-memory state
//! (`SupervisorHandle`), with no Tauri event emission built in -- kept
//! that way deliberately so the core startup/shutdown logic has no Tauri
//! dependency and is directly unit-testable (see `supervisor`'s test
//! suite) without a mock `AppHandle`. The frontend observes progress by
//! polling `get_service_status` on a short interval while not yet
//! `Ready`/`Failed`.

mod supervisor;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use supervisor::{HealthCheckError, HealthResponse, SupervisorHandle, SupervisorState};
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
/// service, on the frontend's behalf, entirely in Rust. See
/// `supervisor::check_service_health`'s docs for the full request/
/// liveness-check contract. Returns a sanitized, fixed error string --
/// never handing the session token, host, port, or a raw HTTP/parse
/// error to the WebView's JavaScript context, which is also the smaller
/// exposed capability versus the alternative (giving the frontend the
/// raw connection details and loosening CSP `connect-src` to let it call
/// the service directly) -- and is also why no CORS configuration is
/// needed on the FastAPI side at all (a same-process Rust HTTP client
/// never sends an `Origin` header, so CORS is not the relevant boundary
/// here -- the session-token check is).
#[tauri::command]
async fn check_service_health(
    state: tauri::State<'_, SupervisorHandle>,
) -> Result<HealthResponse, String> {
    let connection = match state.ready_connection().await {
        Some(connection) => connection,
        None => return Err(HealthCheckError::ServiceNotReady.as_str().to_string()),
    };
    supervisor::check_service_health(&connection)
        .await
        .map_err(|e| e.as_str().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let supervisor_handle = SupervisorHandle::new();
    // S1-04 review finding 2: guards `RunEvent::ExitRequested`, which can
    // fire more than once (e.g. a user clicking the titlebar close button
    // repeatedly before the app actually exits) -- only the *first*
    // firing spawns the shutdown+exit task; every later one is a no-op.
    // `supervisor::shutdown` is independently idempotent too (see its own
    // docs), so this is defense in depth, not the only thing making
    // repeated close requests safe -- but it also avoids spawning a pile
    // of redundant tasks for no reason.
    let shutdown_initiated = Arc::new(AtomicBool::new(false));

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

    app.run(move |app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
            // `ExitRequested` fires twice in the real exit path: once
            // naturally (`code: None`, e.g. the user closed the window)
            // -- the one we want to intercept to run cleanup first -- and
            // again when *this handler's own* `app_handle.exit(0)` call
            // below re-enters the event loop (`code: Some(0)`). Calling
            // `api.prevent_exit()` unconditionally on every
            // `ExitRequested` event was a real bug this project hit: it
            // prevented that second, self-generated exit too, so the
            // process never actually terminated -- confirmed by manual
            // testing (Windows process list showed `app.exe` still
            // running, "Responding", nearly a minute after a clean
            // graceful shutdown had already completed and logged
            // successfully). A `Some(code)` exit request is always our
            // own follow-through, never a fresh request to intercept, so
            // it must be allowed to proceed.
            if code.is_some() {
                return;
            }

            // Prevent the default immediate exit so the graceful
            // shutdown sequence (private-channel `shutdown` message,
            // bounded grace period, then a forced kill if needed) can run
            // to completion first -- "stop accepting frontend access
            // immediately ... wait for a bounded grace period ...
            // forcefully terminate ... if graceful shutdown fails."
            api.prevent_exit();

            if shutdown_initiated.swap(true, Ordering::SeqCst) {
                return; // already handling a previous close request
            }

            let handle = app_handle.state::<SupervisorHandle>().inner().clone();
            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                supervisor::shutdown(&handle).await;
                app_handle.exit(0);
            });
        }
    });
}
