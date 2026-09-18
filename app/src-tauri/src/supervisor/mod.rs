//! S1-04 process supervision: launches the local FastAPI service, proves
//! its identity via the D-025 HMAC challenge, installs a fresh per-launch
//! session credential over the private channel, and only then allows the
//! frontend to reach it. See `docs/DECISIONS.md` D-001/D-009/D-018/D-025
//! and `docs/SPRINT_1_BACKLOG.md` S1-04 for the full contract this
//! implements, and `service/src/evidencegraph_service/supervised.py` for
//! the child's half of the same handshake.
//!
//! State machine ([`SupervisorState`]):
//!
//! ```text
//!     Starting -> AwaitingEndpoint -> VerifyingIdentity
//!              -> InstallingCredential -> Ready -> Failed  [child died/channel failed]
//!     (any state before Ready) -> Failed(reason)   [terminal, fail-closed]
//!     (any state) -> Stopped                       [cancelled/shutdown]
//! ```
//!
//! Every transition to `Failed` means: no endpoint or token is, or ever
//! was, exposed to the frontend ([`SupervisorHandle::ready_connection`]
//! only ever returns `Some` while the state is exactly `Ready`), and the
//! child is killed if it's still running. There is no fallback to a
//! different endpoint, and no retry, at any stage -- a fresh attempt
//! means a fresh [`SupervisorHandle`], a fresh child, a fresh startup
//! secret, nonce, and session token.
//!
//! **A single background task owns the entire lifecycle** ([`run_lifecycle`],
//! spawned once by [`start`]): it runs the startup handshake, and --
//! critically for S1-04 review finding 1 -- *keeps running* after
//! publishing `Ready`, continuously watching the child (`child.wait()`)
//! and reacting to a cancellation request (`shutdown`) via one
//! `tokio::select!`. This is what makes "revoke stale connections after
//! child death" true: there is no point between `Ready` and an explicit
//! `shutdown` where nothing is watching the child. [`SupervisorHandle::
//! arm`]/[`take_cancellation_token`](SupervisorHandle::take_cancellation_token)
//! and the stored `JoinHandle` make [`start`]/[`shutdown`] idempotent
//! (S1-04 review finding 2): a cancelled startup can never subsequently
//! publish `Ready` (the code path that does so is structurally
//! unreachable once the cancellation branch of a `select!` wins -- the
//! losing branch's future, and everything it owned, is simply dropped),
//! and a second `shutdown` call is a no-op because the cancellation token
//! and the task handle can each only be taken once.

mod b64url;
mod challenge;
pub mod errors;
#[cfg(windows)]
mod job_object;
mod process;
mod protocol;
mod state;

pub use errors::{FailureReason, HealthCheckError};
pub use state::{ReadyConnection, SupervisorHandle, SupervisorState};

use rand::RngCore;
use std::path::Path;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::watch;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Overall bound on the entire startup sequence (spawn through `Ready`).
/// Generous for a cold Python interpreter + FastAPI import on a slow
/// machine, but still bounded -- "startup timeout" is a required
/// fail-closed condition, not an unbounded wait.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound on waiting for the child to exit after a graceful shutdown
/// request before force-killing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Bound on waiting for `shutdown`'s best-effort acknowledgement read.
const SHUTDOWN_ACK_TIMEOUT: Duration = Duration::from_secs(2);
/// Bound on `shutdown` waiting for the lifecycle task to fully finish
/// cleanup after cancelling it -- generous relative to `SHUTDOWN_GRACE`
/// so it is never the thing that times out first.
const LIFECYCLE_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

const STARTUP_SECRET_LEN: usize = 32;
const NONCE_LEN: usize = 16;
const SESSION_TOKEN_LEN: usize = 32;

/// S1-04 review finding 3: bounds on every HTTP request this process
/// makes (the D-025 challenge, and `/health`).
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Enforced by reading the response body incrementally and counting
/// actual bytes received -- never by trusting a `Content-Length` header,
/// which a malicious or buggy responder could omit, lie about, or send
/// via chunked encoding without one at all.
const MAX_HTTP_RESPONSE_BYTES: usize = 4096;

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// Debug-build-only diagnostic logging: rich, potentially path/IO-error-
/// bearing detail for a developer's own console, never anything this
/// process persists, logs in a release build, or exposes to the frontend
/// (that's always a fixed [`FailureReason`]/[`HealthCheckError`]). See
/// `errors` module docs.
#[cfg(debug_assertions)]
fn debug_log(context: &str, detail: &impl std::fmt::Display) {
    eprintln!("[supervisor] {context}: {detail}");
}
#[cfg(not(debug_assertions))]
fn debug_log(_context: &str, _detail: &impl std::fmt::Display) {}

/// Typed, closed `GET /health` response schema (matches `service/src/
/// evidencegraph_service/routes/health.py`). `deny_unknown_fields`
/// rejects an unexpected extra field rather than silently ignoring it --
/// S1-04 review finding 3's "validate exact expected response schemas."
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub version: String,
}

/// Typed, closed D-025 challenge response schema.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeResponseBody {
    response: String,
}

/// Starts the supervisor's single lifecycle task. Idempotent: a second
/// call on the same (already-armed) `handle` is a no-op -- see
/// [`SupervisorHandle::arm`].
pub async fn start(app: AppHandle, handle: SupervisorHandle) {
    let token = CancellationToken::new();
    if !handle.arm(token.clone()).await {
        return;
    }
    let handle_for_task = handle.clone();
    let join = tauri::async_runtime::spawn(run_lifecycle(app, handle_for_task, token));
    handle.set_lifecycle_task(join).await;
}

/// Requests shutdown and waits for the lifecycle task to finish cleaning
/// up. Idempotent (S1-04 review finding 2): calling this more than once,
/// concurrently or sequentially, cancels/awaits at most once --
/// `take_cancellation_token`/`take_lifecycle_task` each hand out their
/// value to exactly one caller. A no-op if the supervisor never started
/// or has already fully shut down.
pub async fn shutdown(handle: &SupervisorHandle) {
    let Some(token) = handle.take_cancellation_token().await else {
        return;
    };
    token.cancel();
    if let Some(join) = handle.take_lifecycle_task().await {
        let _ = timeout(LIFECYCLE_JOIN_TIMEOUT, join).await;
    }
}

/// The one task that owns the child process for its entire lifetime, from
/// spawn through final exit -- see module docs for why this single-task
/// design is what makes findings 1 and 2 hold structurally rather than by
/// convention.
async fn run_lifecycle(app: AppHandle, handle: SupervisorHandle, token: CancellationToken) {
    handle.set_state(SupervisorState::Starting).await;

    let (executable, args) = match process::resolve(&app) {
        Ok(v) => v,
        Err(e) => {
            debug_log("process::resolve failed", &e);
            handle
                .set_state(SupervisorState::Failed {
                    reason: process_error_reason(&e).as_str(),
                })
                .await;
            return;
        }
    };

    let raced = tokio::select! {
        result = timeout(STARTUP_TIMEOUT, run_startup_with_executable(&handle, &executable, &args)) => Some(result),
        () = token.cancelled() => None,
    };

    let handshake = match raced {
        None => {
            // Cancelled during startup: the losing `run_startup_with_
            // executable` future was dropped right here by `select!`,
            // which drops everything it owned (the `Child` --
            // `kill_on_drop(true)` -- and, on Windows, the `JobObject`
            // guard). `Ready` is never published: the code path that
            // would do so is later in this function, unreachable from
            // this `match` arm.
            handle.set_state(SupervisorState::Stopped).await;
            return;
        }
        Some(Err(_)) => {
            handle
                .set_state(SupervisorState::Failed {
                    reason: FailureReason::StartupTimeout.as_str(),
                })
                .await;
            return;
        }
        Some(Ok(Err(reason))) => {
            handle
                .set_state(SupervisorState::Failed {
                    reason: reason.as_str(),
                })
                .await;
            return;
        }
        Some(Ok(Ok(result))) => result,
    };

    watch_ready_process(handle, handshake, token).await;
}

/// The second half of the lifecycle, split out from [`run_lifecycle`]
/// specifically so it's directly testable without a Tauri `AppHandle`:
/// publishes `Ready`, then watches the child for the rest of its life,
/// reacting to either an unexpected exit (S1-04 review finding 1) or a
/// cancellation request (graceful shutdown) via one `tokio::select!`.
async fn watch_ready_process(
    handle: SupervisorHandle,
    handshake: HandshakeResult,
    token: CancellationToken,
) {
    let HandshakeResult {
        process,
        host,
        port,
        token: session_token,
        http_client,
    } = handshake;
    let SpawnedProcess {
        mut child,
        mut stdin,
        mut stdout,
        #[cfg(windows)]
        job,
    } = process;

    let (alive_tx, alive_rx) = watch::channel(true);
    handle
        .set_ready(ReadyConnection::new(
            host,
            port,
            session_token,
            http_client,
            alive_rx,
        ))
        .await;

    tokio::select! {
        status = child.wait() => {
            debug_log("child exited unexpectedly", &status.map(|s| s.to_string()).unwrap_or_default());
            // "Stop accepting frontend access immediately": flip the
            // liveness signal before anything else, then reap/invalidate.
            let _ = alive_tx.send(false);
            handle.invalidate_ready(FailureReason::ChildExitedUnexpectedly.as_str()).await;
        }
        () = token.cancelled() => {
            let _ = alive_tx.send(false);
            let _ = protocol::write_shutdown(&mut stdin).await;
            // Best-effort confirmation only -- a missing/slow
            // acknowledgement must never block the real shutdown signal
            // below, which is the child actually exiting.
            let _ = timeout(SHUTDOWN_ACK_TIMEOUT, protocol::read_shutdown_ack(&mut stdout)).await;

            if timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            handle.mark_stopped().await;
        }
    }

    #[cfg(windows)]
    drop(job);
}

struct SpawnedProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    #[cfg(windows)]
    job: job_object::JobObject,
}

struct HandshakeResult {
    process: SpawnedProcess,
    host: String,
    port: u16,
    token: String,
    http_client: reqwest::Client,
}

/// The Tauri-independent core of the startup handshake: spawn, private-
/// channel exchange, D-025 HTTP challenge, credential installation.
/// Returns ownership of the spawned process (for [`run_lifecycle`] to
/// keep watching) and the resulting connection parameters -- it does
/// *not* call `handle.set_ready` itself, so there is exactly one place in
/// this module that ever publishes a `Ready` state.
async fn run_startup_with_executable(
    handle: &SupervisorHandle,
    executable: &Path,
    args: &[String],
) -> Result<HandshakeResult, FailureReason> {
    let mut child = process::spawn(executable, args).map_err(|e| {
        debug_log("process::spawn failed", &e);
        process_error_reason(&e)
    })?;

    // Must be kept alive for exactly as long as the child is meant to
    // keep running -- see `state::ReadyConnection`/`run_lifecycle`: the
    // returned `SpawnedProcess` is what keeps this alive after this
    // function returns. A function-local variable dropped at the end of
    // *this* function (rather than returned) was a real bug this project
    // hit: the job handle closed the instant startup finished, which
    // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` turns into "kill the child
    // right after it reached Ready."
    #[cfg(windows)]
    let job = {
        let job = job_object::JobObject::create().map_err(|e| {
            debug_log("job object create failed", &e);
            FailureReason::JobObjectCreateFailed
        })?;
        job.assign(&child).map_err(|e| {
            debug_log("job object assign failed", &e);
            FailureReason::JobObjectAssignFailed
        })?;
        job
    };

    let mut stdin = child
        .stdin
        .take()
        .ok_or(FailureReason::ChildStdinUnavailable)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or(FailureReason::ChildStdoutUnavailable)?;
    drain_stderr(child.stderr.take());

    let secret = random_bytes(STARTUP_SECRET_LEN);
    protocol::write_startup_secret(&mut stdin, &secret)
        .await
        .map_err(|_| FailureReason::PrivateChannelWriteFailed)?;

    handle.set_state(SupervisorState::AwaitingEndpoint).await;
    // `endpoint.host` is guaranteed `== "127.0.0.1"` here already --
    // `protocol::read_endpoint_ready` enforces that at parse time (S1-04
    // review finding 5), so there is nothing left to re-check.
    let endpoint = race_read(&mut child, protocol::read_endpoint_ready(&mut stdout)).await?;

    handle.set_state(SupervisorState::VerifyingIdentity).await;
    let http_client = build_http_client()?;
    verify_identity(&http_client, &secret, &endpoint.host, endpoint.port).await?;

    handle
        .set_state(SupervisorState::InstallingCredential)
        .await;
    let token = b64url::encode(&random_bytes(SESSION_TOKEN_LEN));
    protocol::write_install_session_token(&mut stdin, &token)
        .await
        .map_err(|_| FailureReason::PrivateChannelWriteFailed)?;
    race_read(&mut child, protocol::read_ready(&mut stdout)).await?;

    Ok(HandshakeResult {
        process: SpawnedProcess {
            child,
            stdin,
            stdout,
            #[cfg(windows)]
            job,
        },
        host: endpoint.host,
        port: endpoint.port,
        token,
        http_client,
    })
}

fn process_error_reason(e: &process::ProcessError) -> FailureReason {
    match e {
        process::ProcessError::ExecutableNotFound(_) => FailureReason::ExecutableNotFound,
        process::ProcessError::ResourceDirUnavailable => FailureReason::ResourceDirUnavailable,
        process::ProcessError::SpawnFailed(_) => FailureReason::SpawnFailed,
    }
}

/// Races reading the next expected protocol frame against the child
/// exiting -- so a child that dies mid-handshake fails fast with a clear
/// reason instead of hanging until the overall `STARTUP_TIMEOUT`.
async fn race_read<T, F>(child: &mut Child, read_future: F) -> Result<T, FailureReason>
where
    F: std::future::Future<Output = Result<T, protocol::ProtocolError>>,
{
    tokio::select! {
        result = read_future => result.map_err(|e| {
            debug_log("private channel read failed", &e);
            // `HostNotLoopback` gets its own distinct, visible reason
            // (S1-04 review finding 5's loopback-host requirement is
            // enforced at parse time in `protocol::read_endpoint_ready`,
            // but a violation is still worth distinguishing from a
            // generic framing failure here); every other `ProtocolError`
            // variant folds into the generic private-channel failure
            // reason -- none of them carry content that would be useful
            // to distinguish further in a frontend-visible/logged string.
            match e {
                protocol::ProtocolError::HostNotLoopback => FailureReason::EndpointNotLoopback,
                _ => FailureReason::PrivateChannelReadFailed,
            }
        }),
        status = child.wait() => {
            debug_log("child exited during startup", &status.map(|s| s.to_string()).unwrap_or_default());
            Err(FailureReason::ChildExitedDuringStartup)
        }
    }
}

fn build_http_client() -> Result<reqwest::Client, FailureReason> {
    reqwest::Client::builder()
        .no_proxy()
        // This client is used at most a handful of times total per app
        // session (the challenge request, then occasional
        // `check_service_health` calls) -- no throughput benefit to
        // pooling, and not reusing connections removes one more variable
        // if a connection-level issue is ever suspected again.
        .pool_max_idle_per_host(0)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_REQUEST_TIMEOUT)
        // Never follow a redirect: this client only ever talks to a
        // loopback endpoint it just verified belongs to its own spawned
        // child, and a redirect response is not something that endpoint
        // has any legitimate reason to send. Following one could send the
        // session token (or, during the challenge phase, evidence of the
        // handshake itself) to an arbitrary `Location`.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| {
            debug_log("http client build failed", &e);
            FailureReason::HttpClientBuildFailed
        })
}

enum BoundedReadError {
    ReadFailed,
    TooLarge,
}

/// Reads a response body incrementally, enforcing `max_bytes` against the
/// number of bytes *actually received* -- never against the `Content-
/// Length` header, which this deliberately never even inspects. A
/// response that never stops sending data (or that lies about its
/// length) is caught here, before any JSON parsing is attempted, not
/// after buffering an unbounded amount of it.
async fn read_bounded_body(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, BoundedReadError> {
    let mut buf = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if buf.len() > max_bytes {
                    return Err(BoundedReadError::TooLarge);
                }
            }
            Ok(None) => return Ok(buf),
            Err(_) => return Err(BoundedReadError::ReadFailed),
        }
    }
}

async fn verify_identity(
    client: &reqwest::Client,
    secret: &[u8],
    host: &str,
    port: u16,
) -> Result<(), FailureReason> {
    let canonical = challenge::canonical_endpoint(host, port);
    let nonce = random_bytes(NONCE_LEN);
    let url = format!("http://{host}:{port}/__startup/challenge");

    let response = client
        .post(&url)
        .json(&serde_json::json!({ "nonce": b64url::encode(&nonce) }))
        .send()
        .await
        .map_err(|e| {
            debug_log("challenge request failed", &e);
            FailureReason::ChallengeRequestFailed
        })?;

    if !response.status().is_success() {
        return Err(FailureReason::ChallengeResponseRejected);
    }

    let body_bytes = read_bounded_body(response, MAX_HTTP_RESPONSE_BYTES)
        .await
        .map_err(|e| match e {
            BoundedReadError::TooLarge => FailureReason::ChallengeResponseTooLarge,
            BoundedReadError::ReadFailed => FailureReason::ChallengeRequestFailed,
        })?;

    let parsed: ChallengeResponseBody = serde_json::from_slice(&body_bytes)
        .map_err(|_| FailureReason::ChallengeResponseMalformed)?;
    let response_bytes =
        b64url::decode(&parsed.response).map_err(|_| FailureReason::ChallengeResponseMalformed)?;
    if response_bytes.len() != challenge::RESPONSE_LENGTH {
        return Err(FailureReason::ChallengeResponseMalformed);
    }

    if !challenge::verify(secret, &canonical, &nonce, &response_bytes) {
        return Err(FailureReason::ChallengeVerificationFailed);
    }
    Ok(())
}

/// Performs a single authenticated `GET /health` request, on the
/// frontend's behalf, entirely in Rust -- see [`crate::check_service_health`]
/// for why this design (a narrowly-scoped command backed by this
/// function) was chosen over letting the WebView call the service
/// directly.
///
/// **S1-04 review finding 1** (the child-death-to-port-reuse race): a
/// child that exits doesn't necessarily free its port *and* have that
/// port claimed by something else in the same instant, but there is a
/// window, however small, between "the socket this process was listening
/// on becomes free" and "this function's request reaches whatever (if
/// anything) is now listening there." A single liveness check immediately
/// before sending the request narrows that window but does not close it:
/// the child can still die *during* the request, and the OS can hand the
/// freed ephemeral port to an unrelated process before the response comes
/// back. This function checks [`ReadyConnection::is_alive`] (backed by a
/// `watch` channel the lifecycle task flips to `false` the instant it
/// observes the child exit -- see `state.rs` module docs) both
/// immediately before sending the request and again immediately after
/// receiving the response, and treats "dead" at *either* point as a
/// failure: the response body is never returned to the frontend as a
/// successful result if the child was ever observed dead around the call.
///
/// This does not claim to close the race to *zero*. A sufficiently fast,
/// already-listening impersonator that answers faster than this
/// process's own liveness signal propagates (one `watch` channel send,
/// reacting to an OS process-exit wait that itself resolves promptly)
/// could theoretically still slip a response through the "before" check
/// and complete before the "after" check observes the death. Closing that
/// residual window completely would require authenticating the
/// *transport* itself on every request (e.g. binding each HTTP
/// connection's identity to a fresh D-025-style challenge, or moving off
/// TCP-with-a-reusable-ephemeral-port entirely) -- a material change to
/// the approved bearer-token-over-HTTP authentication architecture
/// (D-009), out of scope for S1-04. In practice, the realistic exposure
/// is the gap between an OS process-exit notification and these two
/// checks (low single-digit milliseconds), not an attacker-controllable
/// window: nothing can bind the freed port *before* it is actually freed,
/// and the real child still owns it until the moment this function's own
/// liveness signal is already in flight.
pub async fn check_service_health(
    connection: &ReadyConnection,
) -> Result<HealthResponse, HealthCheckError> {
    if !connection.is_alive() {
        return Err(HealthCheckError::ServiceNoLongerAlive);
    }

    let url = format!("http://{}:{}/health", connection.host, connection.port);
    let response = connection
        .http_client
        .get(&url)
        .bearer_auth(&connection.token)
        .send()
        .await
        .map_err(|e| {
            debug_log("health request failed", &e);
            HealthCheckError::RequestFailed
        })?;

    if !response.status().is_success() {
        return Err(HealthCheckError::UnexpectedStatus);
    }

    let body_bytes = read_bounded_body(response, MAX_HTTP_RESPONSE_BYTES)
        .await
        .map_err(|e| match e {
            BoundedReadError::TooLarge => HealthCheckError::ResponseTooLarge,
            BoundedReadError::ReadFailed => HealthCheckError::RequestFailed,
        })?;

    // The "after" check: even if the body above parses as a perfectly
    // valid, successful-looking response, it must not be trusted if the
    // child was observed to have died while we were reading it.
    if !connection.is_alive() {
        return Err(HealthCheckError::ServiceNoLongerAlive);
    }

    serde_json::from_slice(&body_bytes).map_err(|_| HealthCheckError::ResponseMalformed)
}

/// Continuously drains the child's stderr so it can never fill its pipe
/// buffer and block the child (S1-03's structured JSON logs go there).
/// In debug builds, relayed to this process's own stderr for visibility;
/// in release builds, only drained and discarded -- there is no console
/// to show it in anyway, and the requirement is "no secret ... logged",
/// which this satisfies either way since it never inspects the content.
fn drain_stderr<R: AsyncRead + Unpin + Send + 'static>(stderr: Option<R>) {
    let Some(stderr) = stderr else { return };
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(_line)) = lines.next_line().await {
            #[cfg(debug_assertions)]
            eprintln!("[evidencegraph-service] {_line}");
        }
    });
}

#[cfg(test)]
mod tests {
    //! Several kinds of tests here:
    //!
    //! - Real-process integration tests (`full_handshake_...`,
    //!   `wrong_secret_...`) spawn the *actual* `service/.venv` Python
    //!   interpreter running `evidencegraph_service --supervised` (via
    //!   `process::resolve_dev`, the same dev-only resolver `start` uses)
    //!   and drive the real private channel + real D-025 HTTP challenge
    //!   end-to-end. These are this crate's side of the same proof
    //!   `service/tests/test_supervised_process.py` already established
    //!   from the Python side. Requires `uv sync` to have been run in
    //!   `service/` first (see `service/README.md`).
    //! - The mandatory fake-endpoint tests (`fake_http_endpoint_...`)
    //!   stand up a real (but fake/scripted) HTTP responder in place of a
    //!   child's reported endpoint and prove `verify_identity` -- the
    //!   exact function the real handshake calls during
    //!   `VerifyingIdentity` -- rejects it, never proceeding to install a
    //!   session token (that step is unreachable without `verify_identity`
    //!   returning `Ok`, enforced by the `?` in `run_startup_with_executable`
    //!   above, not merely by test convention).
    //! - `child_death_...` tests (S1-04 review finding 1) reproduce the
    //!   exact reported attack: complete a real handshake, kill the child
    //!   from *outside* Rust's ownership of it (`taskkill`, simulating a
    //!   crash or external termination, not a graceful shutdown this
    //!   process initiated), bind a replacement listener on the freed
    //!   port, and prove `check_service_health` never sends it a bearer
    //!   credential and never returns its response as a success.
    //! - `shutdown_...`/`start_...` tests (S1-04 review finding 2) prove
    //!   idempotency and that a cancelled startup can never publish Ready.

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn real_dev_target() -> (std::path::PathBuf, Vec<String>) {
        process::resolve_dev().expect(
            "service/.venv python not found -- run `uv sync` in service/ first \
             (see service/README.md)",
        )
    }

    #[tokio::test]
    async fn full_handshake_against_real_python_child_reaches_ready_and_shuts_down_cleanly() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();

        let result = timeout(
            Duration::from_secs(20),
            run_startup_with_executable(&handle, &executable, &args),
        )
        .await;
        let handshake = match result {
            Ok(Ok(h)) => h,
            Ok(Err(reason)) => panic!("startup should succeed, got failure: {reason:?}"),
            Err(_) => panic!("startup should succeed, timed out"),
        };

        let token = CancellationToken::new();
        let watcher = tokio::spawn(watch_ready_process(
            handle.clone(),
            handshake,
            token.clone(),
        ));

        assert!(
            poll_state(
                &handle,
                |s| *s == SupervisorState::Ready,
                Duration::from_secs(5)
            )
            .await
        );

        let connection = handle
            .ready_connection()
            .await
            .expect("connection present when Ready");
        assert_eq!(connection.host, "127.0.0.1");
        assert!(connection.is_alive());

        let health = check_service_health(&connection)
            .await
            .expect("health check should succeed");
        assert_eq!(health.status, "ok");
        assert_eq!(health.service, "evidencegraph-service");

        // A request with no token must still be rejected -- the token
        // this test holds is not somehow also accepted unauthenticated.
        let unauthed_url = format!("http://{}:{}/health", connection.host, connection.port);
        let unauthed = connection
            .http_client
            .get(&unauthed_url)
            .send()
            .await
            .unwrap();
        assert_eq!(unauthed.status(), 401);

        token.cancel();
        watcher.await.expect("watcher task should not panic");

        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());
        assert!(!connection.is_alive());

        // The port is actually released, not just "the handle says
        // stopped."
        assert!(
            tokio::net::TcpStream::connect((connection.host.as_str(), connection.port))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn wrong_secret_verification_against_a_real_child_is_rejected_and_nothing_is_installed() {
        let (executable, args) = real_dev_target();
        let mut child = process::spawn(&executable, &args).expect("spawn should succeed");
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        drain_stderr(child.stderr.take());

        let real_secret = random_bytes(STARTUP_SECRET_LEN);
        protocol::write_startup_secret(&mut stdin, &real_secret)
            .await
            .unwrap();
        let endpoint = timeout(
            Duration::from_secs(10),
            protocol::read_endpoint_ready(&mut stdout),
        )
        .await
        .unwrap()
        .unwrap();

        // Verify with the *wrong* secret -- behaviorally identical, from
        // the verifier's point of view, to the responder being an
        // impersonator that never had the real one.
        let client = build_http_client().unwrap();
        let wrong_secret = random_bytes(STARTUP_SECRET_LEN);
        let result = verify_identity(&client, &wrong_secret, &endpoint.host, endpoint.port).await;
        assert!(result.is_err());

        // Never send the session token to a child that failed identity
        // verification -- close the channel instead of proceeding.
        drop(stdin);
        let status = timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            !status.success(),
            "child should exit non-zero: never reached readiness"
        );
    }

    async fn fake_http_endpoint(status_line: &str, body: String) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status_line = status_line.to_string();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await; // drain the request; content is irrelevant here
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_wrong_response_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let wrong_response = b64url::encode(&[0u8; 32]);
        let addr =
            fake_http_endpoint("200 OK", format!("{{\"response\":\"{wrong_response}\"}}")).await;

        let client = build_http_client().unwrap();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_malformed_json_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr = fake_http_endpoint("200 OK", "not json at all".to_string()).await;

        let client = build_http_client().unwrap();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_error_status_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr =
            fake_http_endpoint("404 Not Found", "{\"detail\":\"Not Found\"}".to_string()).await;

        let client = build_http_client().unwrap();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_unexpected_field_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let response = b64url::encode(&[0u8; 32]);
        let addr = fake_http_endpoint(
            "200 OK",
            format!("{{\"response\":\"{response}\",\"extra\":\"field\"}}"),
        )
        .await;

        let client = build_http_client().unwrap();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_oversized_body_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let huge = "x".repeat(MAX_HTTP_RESPONSE_BYTES + 1);
        let addr = fake_http_endpoint("200 OK", format!("{{\"response\":\"{huge}\"}}")).await;

        let client = build_http_client().unwrap();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn nonexistent_executable_fails_closed() {
        let handle = SupervisorHandle::new();
        let bogus = std::path::PathBuf::from("Z:\\this\\path\\does\\not\\exist\\evidencegraph.exe");

        let result = run_startup_with_executable(&handle, &bogus, &[]).await;

        assert!(result.is_err());
        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn shutdown_is_a_noop_when_supervisor_never_started() {
        let handle = SupervisorHandle::new();

        shutdown(&handle).await; // must not panic or hang

        assert_eq!(handle.snapshot().await, SupervisorState::Starting);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_when_called_twice_concurrently() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .unwrap();

        let token = CancellationToken::new();
        assert!(handle.arm(token.clone()).await);
        let join =
            tauri::async_runtime::spawn(watch_ready_process(handle.clone(), handshake, token));
        handle.set_lifecycle_task(join).await;

        assert!(
            poll_state(
                &handle,
                |s| *s == SupervisorState::Ready,
                Duration::from_secs(5)
            )
            .await
        );

        // Both calls run concurrently: idempotency means only one of them
        // does the real cancel+join work, and neither panics or hangs.
        let (r1, r2) = tokio::join!(shutdown(&handle), shutdown(&handle));
        let _: ((), ()) = (r1, r2);

        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
    }

    #[tokio::test]
    async fn cancelling_during_startup_never_publishes_ready() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let token = CancellationToken::new();

        // Cancel immediately -- races the handshake from the very start.
        token.cancel();

        let raced = tokio::select! {
            result = timeout(Duration::from_secs(20), run_startup_with_executable(&handle, &executable, &args)) => Some(result),
            () = token.cancelled() => None,
        };

        // Either outcome is acceptable here (the handshake may have
        // already been mid-flight when cancel() was called in a real
        // scheduling), but Ready must never be observable afterward.
        let _ = raced;
        assert!(handle.ready_connection().await.is_none());
    }

    // --- S1-04 review finding 1: child death revokes the connection ------

    #[tokio::test]
    async fn child_death_after_ready_invalidates_the_connection() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .unwrap();
        let pid = handshake
            .process
            .child
            .id()
            .expect("child should have a pid");

        let token = CancellationToken::new();
        let watcher = tokio::spawn(watch_ready_process(handle.clone(), handshake, token));

        assert!(
            poll_state(
                &handle,
                |s| *s == SupervisorState::Ready,
                Duration::from_secs(5)
            )
            .await
        );
        let connection = handle.ready_connection().await.unwrap();
        assert!(connection.is_alive());

        kill_process_externally(pid);

        assert!(
            poll_state(
                &handle,
                |s| matches!(s, SupervisorState::Failed { .. }),
                Duration::from_secs(5)
            )
            .await
        );
        assert!(handle.ready_connection().await.is_none());
        assert!(!connection.is_alive());

        watcher.await.expect("watcher task should not panic");
    }

    #[tokio::test]
    async fn child_death_and_port_reuse_never_leaks_credential_to_replacement_listener() {
        // The exact reported attack: complete a real supervised startup,
        // terminate the service, bind a replacement listener on its
        // released port, invoke the health command again, and assert the
        // replacement listener receives no bearer credential and its
        // response never reaches the caller as a successful health
        // result.
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .unwrap();
        let pid = handshake
            .process
            .child
            .id()
            .expect("child should have a pid");

        let token = CancellationToken::new();
        let watcher = tokio::spawn(watch_ready_process(handle.clone(), handshake, token));

        assert!(
            poll_state(
                &handle,
                |s| *s == SupervisorState::Ready,
                Duration::from_secs(5)
            )
            .await
        );
        let connection = handle.ready_connection().await.unwrap();
        let port = connection.port;

        kill_process_externally(pid);
        assert!(
            poll_state(
                &handle,
                |s| matches!(s, SupervisorState::Failed { .. }),
                Duration::from_secs(5)
            )
            .await
        );

        // Wait for the OS to actually release the port before binding the
        // replacement -- on Windows this is normally immediate once the
        // owning process has exited.
        assert!(
            wait_until(
                || std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
                Duration::from_secs(5)
            )
            .await
        );

        let received_auth_header: std::sync::Arc<tokio::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let received_auth_header_clone = received_auth_header.clone();
        let replacement = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("port should be free");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = replacement.accept().await {
                let mut buf = vec![0u8; 4096];
                if let Ok(n) = socket.read(&mut buf).await {
                    let request_text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let auth_line = request_text
                        .lines()
                        .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                        .map(str::to_string);
                    *received_auth_header_clone.lock().await = auth_line;
                }
                let body = b"{\"status\":\"ok\",\"service\":\"evidencegraph-service\",\"version\":\"0.1.0\"}";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    String::from_utf8_lossy(body)
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        // Give the replacement listener a moment to be ready to accept,
        // then attempt the health check against the stale connection --
        // this must fail closed via the `is_alive()` check, never
        // completing a request against the replacement at all.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let result = check_service_health(&connection).await;

        assert!(
            result.is_err(),
            "a stale connection must never report success"
        );
        assert_eq!(result.unwrap_err(), HealthCheckError::ServiceNoLongerAlive);

        // Give the replacement listener time to have received a
        // connection, if it were going to.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            received_auth_header.lock().await.is_none(),
            "the replacement listener must never receive an Authorization header"
        );

        watcher.await.expect("watcher task should not panic");
    }

    fn kill_process_externally(pid: u32) {
        // Kills the process from *outside* Rust's ownership of the
        // `Child` handle -- `std::process::Command` here spawns a
        // completely separate `taskkill` process targeting the PID,
        // simulating a crash or an external termination, not a graceful
        // shutdown this process itself initiated via `child.kill()`.
        let status = std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        assert!(
            status.is_ok_and(|s| s.success()),
            "taskkill should succeed against our own child"
        );
    }

    async fn poll_state<F: Fn(&SupervisorState) -> bool>(
        handle: &SupervisorHandle,
        predicate: F,
        timeout_duration: Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout_duration;
        loop {
            let state = handle.snapshot().await;
            if predicate(&state) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// General-purpose poll for a plain synchronous condition (e.g. "can
    /// I bind this port yet") where `poll_state` (which is specifically
    /// about `SupervisorHandle`'s async state) doesn't apply.
    async fn wait_until<F: Fn() -> bool>(condition: F, timeout_duration: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout_duration;
        loop {
            if condition() {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
