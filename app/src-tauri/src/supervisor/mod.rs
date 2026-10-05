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
//! publishing `Ready`, continuously watching both the child
//! (`child.wait()`) and the one pinned HTTP connection to it
//! (`pinned.closed()` -- see `pinned_http` module docs and
//! `docs/DECISIONS.md` D-026), and reacting to a cancellation request
//! (`shutdown`) via one `tokio::select!`. This is what makes "revoke
//! stale connections after child death" true: there is no point between
//! `Ready` and an explicit `shutdown` where nothing is watching the
//! child or its connection. [`SupervisorHandle::
//! arm_and_spawn`]/[`begin_shutdown`](SupervisorHandle::begin_shutdown)
//! make [`start`]/[`shutdown`] idempotent (S1-04 review finding 2): the
//! cancellation token and the task handle are registered together and
//! can each only be taken once, so a second `shutdown` call is a no-op.
//!
//! **Shutdown vs. readiness** (S1-04 review round 3, finding 1): the
//! `select!` in [`run_lifecycle`] only covers shutdown arriving *during*
//! the handshake. Shutdown can also arrive after the handshake has won
//! that race but before `Ready` is published. Readiness publication and
//! shutdown recording are therefore decided under the same
//! [`SupervisorHandle`] lock: [`begin_shutdown`](SupervisorHandle::
//! begin_shutdown) records the request and revokes any published
//! connection in one step, and [`set_ready`](SupervisorHandle::set_ready)
//! refuses to publish once that record exists. Whichever takes the lock
//! first wins; there is no separate check-then-publish step for the
//! other to slip between. If shutdown wins, the lifecycle stops the child
//! without ever exposing a connection.

mod b64url;
mod challenge;
pub mod errors;
#[cfg(windows)]
mod job_object;
mod pinned_http;
mod process;
mod protocol;
mod state;

pub use errors::{FailureReason, HealthCheckError};
pub use state::{ReadyConnection, SupervisorHandle, SupervisorState};

use bytes::Bytes;
use http_body_util::Full;
use hyper::Request;
use pinned_http::PinnedConnection;
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
/// [`SupervisorHandle::arm_and_spawn`].
pub async fn start(app: AppHandle, handle: SupervisorHandle) {
    let token = CancellationToken::new();
    let handle_for_task = handle.clone();
    let token_for_task = token.clone();
    // Registering the token and the task handle in one locked step means
    // a concurrent `shutdown` either sees neither (and this then refuses
    // to start, because shutdown was recorded first) or both (and waits
    // for the task) -- never a cancelled token with no task to await.
    handle
        .arm_and_spawn(token, move || {
            tauri::async_runtime::spawn(run_lifecycle(app, handle_for_task, token_for_task))
        })
        .await;
}

/// Requests shutdown and waits for the lifecycle task to finish cleaning
/// up. Idempotent (S1-04 review finding 2): calling this more than once,
/// concurrently or sequentially, cancels/awaits at most once --
/// [`SupervisorHandle::begin_shutdown`] hands the token and task to
/// exactly one caller. Always records the request first, so even a call
/// before the supervisor started (or racing readiness publication)
/// prevents any later `Ready` -- see module docs.
pub async fn shutdown(handle: &SupervisorHandle) {
    let Some((token, join)) = handle.begin_shutdown().await else {
        return;
    };
    token.cancel();
    let _ = timeout(LIFECYCLE_JOIN_TIMEOUT, join).await;
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
/// publishes `Ready` (unless shutdown was already recorded -- see module
/// docs), then watches the child for the rest of its life, reacting to
/// either an unexpected exit (S1-04 review finding 1) or a cancellation
/// request (graceful shutdown) via one `tokio::select!`.
async fn watch_ready_process(
    handle: SupervisorHandle,
    handshake: HandshakeResult,
    token: CancellationToken,
) {
    watch_ready_process_inner(handle, handshake, token, PrePublishHook::noop()).await;
}

/// Test-only synchronization point immediately before readiness
/// publication: lets a test park the lifecycle after the handshake has
/// completed, record shutdown, and only then let publication proceed --
/// forcing the startup-complete/publication boundary deterministically.
/// Zero-sized and a no-op outside `cfg(test)`.
#[derive(Default)]
struct PrePublishHook {
    #[cfg(test)]
    pause: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}

impl PrePublishHook {
    fn noop() -> Self {
        Self::default()
    }

    async fn fire(self) {
        #[cfg(test)]
        if let Some((reached, resume)) = self.pause {
            let _ = reached.send(());
            let _ = resume.await;
        }
    }
}

async fn watch_ready_process_inner(
    handle: SupervisorHandle,
    handshake: HandshakeResult,
    token: CancellationToken,
    hook: PrePublishHook,
) {
    let HandshakeResult {
        process,
        host,
        port,
        token: session_token,
        pinned,
    } = handshake;
    let SpawnedProcess {
        mut child,
        mut stdin,
        mut stdout,
        #[cfg(windows)]
        job,
    } = process;

    hook.fire().await;

    let (alive_tx, alive_rx) = watch::channel(true);
    let published = handle
        .set_ready(
            ReadyConnection::new(host, port, session_token, pinned.clone(), alive_rx),
            alive_tx,
        )
        .await;
    if !published {
        // Shutdown was recorded before publication could happen: nothing
        // was exposed. Stop the child the same way a normal shutdown does.
        stop_child(&mut child, &mut stdin, &mut stdout).await;
        handle.mark_stopped().await;
        #[cfg(windows)]
        drop(job);
        return;
    }

    tokio::select! {
        // `biased`, cancellation first: `begin_shutdown` cancels the token
        // before it revokes the transport, so when shutdown is what closed
        // the connection, this branch is always the one taken.
        biased;
        () = token.cancelled() => {
            // `begin_shutdown` already revoked the connection, down to its
            // transport, when it recorded the request; this is the
            // process-side cleanup.
            stop_child(&mut child, &mut stdin, &mut stdout).await;
            handle.mark_stopped().await;
        }
        status = child.wait() => {
            debug_log("child exited unexpectedly", &status.map(|s| s.to_string()).unwrap_or_default());
            // "Stop accepting frontend access immediately":
            // `invalidate_ready` flips the liveness signal and clears the
            // connection in one locked step.
            handle.invalidate_ready(FailureReason::ChildExitedUnexpectedly.as_str()).await;
        }
        () = pinned.closed() => {
            // S1-04 review round 2, finding 1: the pinned connection can
            // in principle be lost (reset, protocol error, clean close)
            // without the OS having yet delivered this process's own
            // `child.wait()` notification -- react to *either* signal,
            // proactively, rather than only discovering a dead connection
            // reactively the next time something tries to use it.
            debug_log("pinned connection to child lost", &"");
            handle.invalidate_ready(FailureReason::AuthenticatedConnectionLost.as_str()).await;
        }
    }

    #[cfg(windows)]
    drop(job);
}

/// Graceful stop: private-channel `shutdown`, bounded wait, then a forced
/// kill if the child hasn't exited.
async fn stop_child(child: &mut Child, stdin: &mut ChildStdin, stdout: &mut ChildStdout) {
    let _ = protocol::write_shutdown(stdin).await;
    // Best-effort confirmation only -- a missing/slow acknowledgement must
    // never block the real shutdown signal below, which is the child
    // actually exiting.
    let _ = timeout(SHUTDOWN_ACK_TIMEOUT, protocol::read_shutdown_ack(stdout)).await;

    if timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
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
    pinned: PinnedConnection,
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
    // S1-04 review round 2, finding 1: dial the child's endpoint *here* --
    // exactly once for this handshake -- and verify identity over this
    // exact connection. `pinned` is then carried all the way through to
    // `HandshakeResult`/`ReadyConnection` and reused for every later
    // authenticated request; nothing after this point ever calls
    // `PinnedConnection::connect` again. See `pinned_http` module docs and
    // `docs/DECISIONS.md` D-026.
    let pinned = PinnedConnection::connect(&endpoint.host, endpoint.port, HTTP_CONNECT_TIMEOUT)
        .await
        .map_err(|e| {
            debug_log("pinned connection failed", &e);
            FailureReason::ChallengeRequestFailed
        })?;
    verify_identity(&pinned, &secret, &endpoint.host, endpoint.port).await?;

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
        pinned,
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

/// Builds a request against the pinned connection's own endpoint. `host`/
/// `port` are only used for the mandatory HTTP/1.1 `Host` header --
/// `hyper`'s low-level client does not add one automatically the way
/// `reqwest` did.
fn build_request(
    method: &str,
    host: &str,
    port: u16,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> Request<Full<Bytes>> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", format!("{host}:{port}"));
    if let Some(content_type) = content_type {
        builder = builder.header("Content-Type", content_type);
    }
    builder
        .body(Full::new(Bytes::from(body)))
        .expect("request built from a fixed, valid set of headers")
}

async fn verify_identity(
    pinned: &PinnedConnection,
    secret: &[u8],
    host: &str,
    port: u16,
) -> Result<(), FailureReason> {
    let canonical = challenge::canonical_endpoint(host, port);
    let nonce = random_bytes(NONCE_LEN);
    let body = serde_json::to_vec(&serde_json::json!({ "nonce": b64url::encode(&nonce) })).unwrap();
    let request = build_request(
        "POST",
        host,
        port,
        "/__startup/challenge",
        Some("application/json"),
        body,
    );

    // One deadline covers the request and the whole bounded body read
    // (S1-04 review round 3, finding 2 -- see `pinned_http` module docs).
    let response = pinned
        .send(request, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
        .await
        .map_err(|e| {
            debug_log("challenge request failed", &e);
            match e {
                pinned_http::PinnedSendError::BodyTooLarge => {
                    FailureReason::ChallengeResponseTooLarge
                }
                _ => FailureReason::ChallengeRequestFailed,
            }
        })?;

    if !response.status.is_success() {
        return Err(FailureReason::ChallengeResponseRejected);
    }

    let parsed: ChallengeResponseBody = serde_json::from_slice(&response.body)
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
/// **S1-04 review finding 1, round 2** (the child-death-to-port-reuse
/// credential-disclosure race): the original mitigation here checked a
/// liveness flag immediately before and after dialing a *fresh*
/// connection per request. The review correctly rejected that as
/// insufficient -- "checking a liveness flag before or after sending" and
/// dialing again regardless is still two separate steps with a window
/// between them, however small, in which a replacement listener could
/// have claimed the freed port. This function no longer dials anything at
/// all: `connection.pinned` is the *exact* TCP connection over which the
/// D-025 challenge response was cryptographically verified during
/// startup (see `run_startup_with_executable`), kept open and reused for
/// every request since. There is no code path in this module, or in
/// [`pinned_http`], that calls `PinnedConnection::connect` a second time
/// for an already-established connection. If the child dies -- at any
/// point, including *during* this exact request -- the OS-level write/
/// read on this already-open, already-authenticated socket fails (or the
/// connection is independently observed closed by `watch_ready_process`'s
/// own `pinned.closed()` race, which runs concurrently with this
/// function and revokes `Ready` on its own): either way, there is no
/// possible outcome in which this function's request is ever delivered
/// to, or a response ever accepted from, anything other than the one
/// process whose identity was verified at connection time. A replacement
/// listener bound on the freed port receives *nothing* from this
/// function, structurally, not probabilistically -- this process's
/// client-side socket handle refers to the old, dead connection and
/// nothing reopens it. See `pinned_http` module docs and
/// `docs/DECISIONS.md` D-026 for the full design and why the prior
/// `reqwest`-based, dial-per-request approach could not give this
/// guarantee no matter how its liveness checks were tuned.
pub async fn check_service_health(
    connection: &ReadyConnection,
) -> Result<HealthResponse, HealthCheckError> {
    check_service_health_inner(connection, PostLivenessHook::noop()).await
}

/// Test-only synchronization point between the initial liveness pre-check
/// and the actual send, used by the `child_death_*` tests below to force
/// the exact sequence the review specified *deterministically*: the health
/// operation reports (over a channel, not a sleep) that it has passed its
/// pre-check, then blocks until the test has killed the child and bound a
/// replacement listener and explicitly resumes it. A production call takes
/// the no-op path: the type is zero-sized outside `cfg(test)` and `fire`
/// returns immediately.
#[derive(Default)]
struct PostLivenessHook {
    #[cfg(test)]
    pause: Option<TestPause>,
}

#[cfg(test)]
struct TestPause {
    reached: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
    /// Skip `PinnedConnection`'s own liveness-flag fast path on resume, so
    /// the request is written using only the connection itself -- proving
    /// the guarantee does not depend on any flag being fresh.
    bypass_liveness_flag: bool,
}

impl PostLivenessHook {
    fn noop() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn pause(
        reached: tokio::sync::oneshot::Sender<()>,
        resume: tokio::sync::oneshot::Receiver<()>,
        bypass_liveness_flag: bool,
    ) -> Self {
        Self {
            pause: Some(TestPause {
                reached,
                resume,
                bypass_liveness_flag,
            }),
        }
    }

    /// Returns whether the caller should bypass the liveness-flag fast
    /// path (always `false` outside tests).
    async fn fire(self) -> bool {
        #[cfg(test)]
        if let Some(pause) = self.pause {
            let _ = pause.reached.send(());
            let _ = pause.resume.await;
            return pause.bypass_liveness_flag;
        }
        false
    }
}

async fn check_service_health_inner(
    connection: &ReadyConnection,
    hook: PostLivenessHook,
) -> Result<HealthResponse, HealthCheckError> {
    if !connection.is_alive() {
        return Err(HealthCheckError::ServiceNoLongerAlive);
    }

    let bypass_liveness_flag = hook.fire().await;

    let request = Request::builder()
        .method("GET")
        .uri("/health")
        .header("Host", format!("{}:{}", connection.host, connection.port))
        .header("Authorization", format!("Bearer {}", connection.token))
        .body(Full::new(Bytes::new()))
        .expect("request built from a fixed, valid set of headers");

    // One deadline covers waiting for the connection (another health call
    // may hold it), the request, and the whole bounded body read (S1-04
    // review round 3, finding 2 -- see `pinned_http` module docs).
    #[cfg(test)]
    let sent = if bypass_liveness_flag {
        connection
            .pinned
            .send_ignoring_liveness_flag(request, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
            .await
    } else {
        connection
            .pinned
            .send(request, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
            .await
    };
    #[cfg(not(test))]
    let sent = {
        let _ = bypass_liveness_flag;
        connection
            .pinned
            .send(request, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
            .await
    };

    let response = sent.map_err(|e| {
        debug_log("health request failed", &e);
        match e {
            pinned_http::PinnedSendError::BodyTooLarge => HealthCheckError::ResponseTooLarge,
            _ => HealthCheckError::RequestFailed,
        }
    })?;

    if !response.status.is_success() {
        return Err(HealthCheckError::UnexpectedStatus);
    }

    // The connection cannot have silently reconnected to anything else in
    // between -- but it can have gone from alive to dead while this
    // request was in flight, which is still worth surfacing distinctly
    // rather than returning a response that happened to parse.
    if !connection.is_alive() {
        return Err(HealthCheckError::ServiceNoLongerAlive);
    }

    serde_json::from_slice(&response.body).map_err(|_| HealthCheckError::ResponseMalformed)
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
    //! - `child_death_...` tests (S1-04 review finding 1, round 2) reproduce
    //!   the exact reported attack: complete a real handshake, kill the
    //!   child from *outside* Rust's ownership of it (`taskkill`,
    //!   simulating a crash or external termination, not a graceful
    //!   shutdown this process initiated), bind a replacement listener on
    //!   the freed port, and prove `check_service_health` never sends it a
    //!   bearer credential and never returns its response as a success.
    //!   `child_death_between_liveness_check_and_send_never_leaks_
    //!   credential_to_replacement_listener` additionally forces the exact
    //!   sequence the review specified -- kill happening *between* the
    //!   liveness pre-check and the actual send -- deterministically, via
    //!   `PostLivenessHook`'s `oneshot`-channel synchronization point,
    //!   rather than relying on sleeps/timing to land the race a
    //!   particular way. See `pinned_http`'s own test module for the
    //!   lower-level proof that `PinnedConnection` itself never redials.
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

        // A second call reuses the *same* pinned connection (no new
        // `TcpStream::connect` happens anywhere in this path) and still
        // succeeds -- proving ordinary repeated polling works under the
        // never-redial design, not just a single request.
        let health_again = check_service_health(&connection)
            .await
            .expect("second health check over the reused connection should succeed");
        assert_eq!(health_again.status, "ok");

        // A request with no token must still be rejected -- the token
        // this test holds is not somehow also accepted unauthenticated.
        // Sent over the same pinned connection deliberately, to also
        // prove the server-side auth check is per-request, not something
        // the persistent connection itself bypasses.
        let unauthed_request = Request::builder()
            .method("GET")
            .uri("/health")
            .header("Host", format!("{}:{}", connection.host, connection.port))
            .body(Full::new(Bytes::new()))
            .unwrap();
        let unauthed = connection
            .pinned
            .send(
                unauthed_request,
                MAX_HTTP_RESPONSE_BYTES,
                HTTP_REQUEST_TIMEOUT,
            )
            .await
            .unwrap();
        assert_eq!(unauthed.status, 401);

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
    async fn pinned_connection_survives_idling_past_uvicorns_default_keep_alive() {
        // uvicorn's `timeout_keep_alive` defaults to 5s: an idle HTTP/1.1
        // connection is closed by the *server*. The pinned connection is
        // meant to live for the whole session with no reconnect path, so a
        // healthy child that closes an idle connection would otherwise
        // spuriously revoke `Ready`. Idle well past 5s, then require both
        // that `Ready` is still held and that health succeeds.
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .unwrap();
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
        let connection = handle.ready_connection().await.unwrap();

        tokio::time::sleep(Duration::from_secs(7)).await;

        assert_eq!(handle.snapshot().await, SupervisorState::Ready);
        assert!(connection.is_alive());
        let health = check_service_health(&connection)
            .await
            .expect("health must succeed on the retained connection after idling");
        assert_eq!(health.status, "ok");

        token.cancel();
        watcher.await.unwrap();
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
        let pinned = PinnedConnection::connect(&endpoint.host, endpoint.port, HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let wrong_secret = random_bytes(STARTUP_SECRET_LEN);
        let result = verify_identity(&pinned, &wrong_secret, &endpoint.host, endpoint.port).await;
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

        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let result = verify_identity(&pinned, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_malformed_json_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr = fake_http_endpoint("200 OK", "not json at all".to_string()).await;

        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let result = verify_identity(&pinned, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_error_status_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr =
            fake_http_endpoint("404 Not Found", "{\"detail\":\"Not Found\"}".to_string()).await;

        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let result = verify_identity(&pinned, &secret, "127.0.0.1", addr.port()).await;

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

        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let result = verify_identity(&pinned, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_oversized_body_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let huge = "x".repeat(MAX_HTTP_RESPONSE_BYTES + 1);
        let addr = fake_http_endpoint("200 OK", format!("{{\"response\":\"{huge}\"}}")).await;

        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let result = verify_identity(&pinned, &secret, "127.0.0.1", addr.port()).await;

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
        let (handle_for_task, token_for_task) = (handle.clone(), token.clone());
        assert!(
            handle
                .arm_and_spawn(token, move || {
                    tauri::async_runtime::spawn(watch_ready_process(
                        handle_for_task,
                        handshake,
                        token_for_task,
                    ))
                })
                .await
        );

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

    // --- S1-04 review round 3, finding 1: shutdown vs. publication -------

    /// Forces the exact boundary from the review: the handshake has
    /// completed (startup won `select!`) and the lifecycle is about to
    /// publish `Ready`, when shutdown is recorded. Driven through the real
    /// `shutdown` function and the real publication path
    /// (`watch_ready_process_inner`), with channels rather than sleeps.
    #[tokio::test]
    async fn shutdown_at_the_publication_boundary_never_publishes_ready() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .expect("startup should succeed");
        let port = handshake.port;
        let pid = handshake
            .process
            .child
            .id()
            .expect("child should have a pid");

        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<()>();
        let token = CancellationToken::new();
        let (handle_for_task, token_for_task) = (handle.clone(), token.clone());
        assert!(
            handle
                .arm_and_spawn(token.clone(), move || {
                    tauri::async_runtime::spawn(watch_ready_process_inner(
                        handle_for_task,
                        handshake,
                        token_for_task,
                        PrePublishHook {
                            pause: Some((reached_tx, resume_rx)),
                        },
                    ))
                })
                .await
        );

        // Startup is complete and the lifecycle is parked right before
        // publication.
        reached_rx
            .await
            .expect("lifecycle must reach the publication point");
        assert_ne!(handle.snapshot().await, SupervisorState::Ready);

        // Shutdown wins the boundary: run the real `shutdown`, and wait
        // until it has recorded the request (it cancels the token only
        // after recording it under the handle's lock).
        let shutdown_handle = handle.clone();
        let shutdown_task = tokio::spawn(async move { shutdown(&shutdown_handle).await });
        token.cancelled().await;

        // Now let publication proceed.
        let _ = resume_tx.send(());
        timeout(Duration::from_secs(15), shutdown_task)
            .await
            .expect("shutdown must finish")
            .expect("shutdown must not panic");

        assert!(
            !handle.ever_published_ready().await,
            "Ready must never be published once shutdown is recorded"
        );
        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());

        // The child was stopped, not left running: its port is released
        // and its pid is gone.
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err());
        assert!(!process_is_running(pid));
    }

    /// The opposite ordering at the same boundary: `Ready` is published
    /// first, then shutdown is recorded. A connection obtained while Ready
    /// must be revoked at the moment shutdown is recorded -- not only once
    /// the lifecycle task gets around to stopping the child.
    #[tokio::test]
    async fn shutdown_recorded_after_publication_revokes_the_connection_immediately() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .unwrap();
        let token = CancellationToken::new();
        let (handle_for_task, token_for_task) = (handle.clone(), token.clone());
        assert!(
            handle
                .arm_and_spawn(token, move || {
                    tauri::async_runtime::spawn(watch_ready_process(
                        handle_for_task,
                        handshake,
                        token_for_task,
                    ))
                })
                .await
        );
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

        // Record shutdown directly (the first half of `shutdown`) and check
        // the revocation the moment it returns, before the lifecycle task
        // has stopped the child.
        let (cancel, join) = handle.begin_shutdown().await.unwrap();
        assert!(!connection.pinned.is_alive(), "the transport is revoked");
        assert!(handle.ready_connection().await.is_none());
        assert!(!connection.is_alive());
        assert_eq!(
            check_service_health(&connection).await.unwrap_err(),
            HealthCheckError::ServiceNoLongerAlive
        );

        cancel.cancel();
        timeout(LIFECYCLE_JOIN_TIMEOUT, join)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
    }

    // --- S1-04 review round 4: shutdown revokes the transport -----------

    /// Stands in for the verified peer of the pinned connection, so a
    /// request can be *held* on it. Accepts connections (counting them),
    /// holds the first request until released, and records every byte
    /// received after that first request -- answering any further
    /// complete request, so that if a second request ever were sent, the
    /// exchange would complete and the bytes would be observable.
    struct HeldPeer {
        port: u16,
        first_request_received: tokio::sync::oneshot::Receiver<()>,
        release_first: Option<tokio::sync::oneshot::Sender<()>>,
        after_first: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl HeldPeer {
        async fn start() -> Self {
            const HEALTH: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 67\r\n\r\n{\"status\":\"ok\",\"service\":\"evidencegraph-service\",\"version\":\"0.1.0\"}";
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let (received_tx, first_request_received) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
            let after_first = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let (after, count) = (after_first.clone(), accepted.clone());
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Any later connection attempt would also be counted.
                tokio::spawn(async move {
                    while listener.accept().await.is_ok() {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                });
                let mut pending = Vec::new();
                let mut buf = vec![0u8; 4096];
                while !pending.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "connection closed before the first request");
                    pending.extend_from_slice(&buf[..n]);
                }
                let end = pending.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                after.lock().unwrap().extend_from_slice(&pending[end..]);
                let _ = received_tx.send(());
                let _ = release_rx.await;
                let _ = socket.write_all(HEALTH.as_bytes()).await;
                let mut answered = 0;
                loop {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let complete = {
                                let mut after = after.lock().unwrap();
                                after.extend_from_slice(&buf[..n]);
                                after.windows(4).filter(|w| *w == b"\r\n\r\n").count()
                            };
                            for _ in answered..complete {
                                let _ = socket.write_all(HEALTH.as_bytes()).await;
                            }
                            answered = complete;
                        }
                    }
                }
            });
            Self {
                port,
                first_request_received,
                release_first: Some(release_tx),
                after_first,
                accepted,
            }
        }

        fn bytes_after_first(&self) -> Vec<u8> {
            self.after_first.lock().unwrap().clone()
        }
    }

    /// The review's sequence, forced with explicit synchronization (no
    /// sleeps decide ordering), through production `shutdown` and the
    /// production lifecycle (`watch_ready_process`) with a real child:
    ///
    /// 1. request A owns the connection (the peer has received it and holds it);
    /// 2. request B passes its liveness check, then queues behind A;
    /// 3. production `shutdown` records revocation;
    /// 4. A is released;
    /// 5. B must send nothing -- no request bytes, no credential.
    ///
    /// The real child is started and verified as usual; only the published
    /// transport is a connection to `HeldPeer`, because the real service
    /// answers `/health` immediately and so cannot hold request A.
    #[tokio::test]
    async fn shutdown_while_a_request_is_queued_sends_nothing_from_it() {
        let (executable, args) = real_dev_target();
        let handle = SupervisorHandle::new();
        let mut handshake = run_startup_with_executable(&handle, &executable, &args)
            .await
            .expect("startup should succeed");
        let pid = handshake
            .process
            .child
            .id()
            .expect("child should have a pid");
        let mut peer = HeldPeer::start().await;
        handshake.pinned = PinnedConnection::connect("127.0.0.1", peer.port, HTTP_CONNECT_TIMEOUT)
            .await
            .unwrap();

        let token = CancellationToken::new();
        let (handle_for_task, token_for_task) = (handle.clone(), token.clone());
        assert!(
            handle
                .arm_and_spawn(token.clone(), move || {
                    tauri::async_runtime::spawn(watch_ready_process(
                        handle_for_task,
                        handshake,
                        token_for_task,
                    ))
                })
                .await
        );
        assert!(
            poll_state(
                &handle,
                |s| *s == SupervisorState::Ready,
                Duration::from_secs(5)
            )
            .await
        );
        let connection = handle.ready_connection().await.unwrap();

        // Step 1: A is on the wire and owns the sender.
        let request_a = {
            let connection = connection.clone();
            tokio::spawn(async move { check_service_health(&connection).await })
        };
        (&mut peer.first_request_received)
            .await
            .expect("request A must reach the peer");

        // Step 2: B passes its initial liveness check (signalled by the
        // hook), then is resumed and parks waiting for the sender
        // (signalled by the transport's queue probe).
        let queued = connection.pinned.notify_when_next_send_queues();
        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<()>();
        let request_b = {
            let connection = connection.clone();
            tokio::spawn(async move {
                check_service_health_inner(
                    &connection,
                    PostLivenessHook::pause(reached_tx, resume_rx, false),
                )
                .await
            })
        };
        reached_rx.await.expect("B must pass its liveness check");
        let _ = resume_tx.send(());
        queued.await.expect("B must queue behind A");
        assert!(connection.is_alive(), "B queued on a live connection");

        // Step 3: production shutdown records revocation. It cancels the
        // token inside the same locked step that revokes; taking the
        // handle's lock afterwards proves that step has finished.
        let shutdown_task = {
            let handle = handle.clone();
            tokio::spawn(async move { shutdown(&handle).await })
        };
        token.cancelled().await;
        let _ = handle.snapshot().await;
        assert!(handle.ready_connection().await.is_none());
        assert!(!connection.is_alive());

        // Step 4: release A.
        let _ = peer.release_first.take().unwrap().send(());

        // Step 5: B sends nothing.
        let b = timeout(Duration::from_secs(5), request_b)
            .await
            .expect("B must finish promptly")
            .unwrap();
        assert!(b.is_err(), "B must fail, got {b:?}");
        let a = timeout(Duration::from_secs(5), request_a)
            .await
            .expect("A must finish promptly")
            .unwrap();
        assert!(
            a.is_err(),
            "A was in flight when the transport was revoked, got {a:?}"
        );
        // B has finished. Had it sent its request, the peer would already
        // have recorded it (the peer reads a request before answering).
        let after_b = peer.bytes_after_first();
        assert!(
            after_b.is_empty(),
            "B sent {} bytes after shutdown was recorded (credential included: {}): {:?}",
            after_b.len(),
            String::from_utf8_lossy(&after_b).contains(&connection.token),
            String::from_utf8_lossy(&after_b)
        );

        // Stale handles reject later calls -- including one that skips
        // every liveness check and goes straight to the transport.
        assert_eq!(
            check_service_health(&connection).await.unwrap_err(),
            HealthCheckError::ServiceNoLongerAlive
        );
        let stale = Request::builder()
            .method("GET")
            .uri("/health")
            .header("Host", format!("127.0.0.1:{}", peer.port))
            .header("Authorization", format!("Bearer {}", connection.token))
            .body(Full::new(Bytes::new()))
            .unwrap();
        assert!(connection
            .pinned
            .send_ignoring_liveness_flag(stale, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
            .await
            .is_err());

        // Cleanup completes: graceful stop, child gone, nothing exposed.
        timeout(Duration::from_secs(15), shutdown_task)
            .await
            .expect("shutdown must finish")
            .unwrap();
        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());
        assert!(!process_is_running(pid));

        // Give any (incorrectly) written bytes time to arrive, then check
        // that nothing at all was received after request A.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after = peer.bytes_after_first();
        assert!(
            after.is_empty(),
            "the peer received {} bytes after request A: {:?}",
            after.len(),
            String::from_utf8_lossy(&after)
        );
        assert!(!String::from_utf8_lossy(&after).contains(&connection.token));
        assert_eq!(
            peer.accepted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "no redial"
        );
        // Structural cause of the above: shutdown revoked the transport
        // itself, not only the handle and the liveness signal.
        assert!(!connection.pinned.is_alive(), "the transport is revoked");
    }

    fn process_is_running(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist should run");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
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
    /// Stands in for an attacker (or any unrelated process) that has
    /// claimed the port the legitimate child released. Records every
    /// connection and every raw byte it receives, and answers any request
    /// with a plausible successful health body -- so if the client ever
    /// did talk to it, the failure mode under test (a "successful" health
    /// result from an impostor) would be reachable, making a passing
    /// assertion meaningful.
    struct ReplacementListener {
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        received: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl ReplacementListener {
        /// Binds `port`, retrying until the OS has released it.
        async fn bind(port: u16) -> Self {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let listener = loop {
                match TcpListener::bind(("127.0.0.1", port)).await {
                    Ok(listener) => break listener,
                    Err(e) => {
                        assert!(
                            tokio::time::Instant::now() < deadline,
                            "port {port} was never released: {e}"
                        );
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }
            };
            let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let (a, r) = (accepted.clone(), received.clone());
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let r = r.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        if let Ok(n) = socket.read(&mut buf).await {
                            r.lock().unwrap().extend_from_slice(&buf[..n]);
                        }
                        let body = "{\"status\":\"ok\",\"service\":\"evidencegraph-service\",\"version\":\"0.1.0\"}";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    });
                }
            });
            Self { accepted, received }
        }

        fn accepted(&self) -> usize {
            self.accepted.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn received_contains(&self, needle: &str) -> bool {
            let bytes = self.received.lock().unwrap();
            String::from_utf8_lossy(&bytes).contains(needle)
        }

        fn received_len(&self) -> usize {
            self.received.lock().unwrap().len()
        }

        /// The core assertion of every test that uses this type.
        fn assert_untouched(&self, session_token: &str) {
            assert_eq!(self.accepted(), 0, "replacement listener saw a connection");
            assert_eq!(
                self.received_len(),
                0,
                "replacement listener received bytes"
            );
            assert!(
                !self.received_contains(session_token),
                "replacement listener received the session token"
            );
        }
    }

    struct EstablishedChild {
        handle: SupervisorHandle,
        connection: ReadyConnection,
        pid: u32,
        watcher: tokio::task::JoinHandle<()>,
    }

    /// Step 1 of the review's sequence: a real supervised child, fully
    /// verified, in `Ready`, with the watcher task running.
    async fn establish_ready_child() -> EstablishedChild {
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
        let watcher = tokio::spawn(watch_ready_process(
            handle.clone(),
            handshake,
            CancellationToken::new(),
        ));
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
        EstablishedChild {
            handle,
            connection,
            pid,
            watcher,
        }
    }

    /// Everything that must be true once the authenticated connection is
    /// gone: Ready revoked, no connection exposed, stale handle dead.
    async fn assert_access_revoked(child: &EstablishedChild) {
        match child.handle.snapshot().await {
            SupervisorState::Failed { reason } => assert!(
                reason == FailureReason::ChildExitedUnexpectedly.as_str()
                    || reason == FailureReason::AuthenticatedConnectionLost.as_str(),
                "unexpected failure reason: {reason}"
            ),
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(child.handle.ready_connection().await.is_none());
        assert!(!child.connection.is_alive());
    }

    async fn wait_for_failed(handle: &SupervisorHandle) {
        assert!(
            poll_state(
                handle,
                |s| matches!(s, SupervisorState::Failed { .. }),
                Duration::from_secs(5)
            )
            .await
        );
    }

    #[tokio::test]
    async fn child_death_and_port_reuse_never_leaks_credential_to_replacement_listener() {
        // The originally reported attack: verified child dies, a
        // replacement takes its port, the health command is invoked again.
        let child = establish_ready_child().await;
        let port = child.connection.port;

        kill_process_externally(child.pid);
        wait_for_failed(&child.handle).await;
        let replacement = ReplacementListener::bind(port).await;

        // Both the stale handle a caller already holds and a fresh lookup
        // must fail closed.
        let result = check_service_health(&child.connection).await;
        assert_eq!(result.unwrap_err(), HealthCheckError::ServiceNoLongerAlive);
        assert!(child.handle.ready_connection().await.is_none());

        tokio::time::sleep(Duration::from_millis(200)).await;
        replacement.assert_untouched(&child.connection.token);
        assert_access_revoked(&child).await;
        child.watcher.await.expect("watcher task should not panic");
    }

    /// The review's required sequence, forced deterministically (channels,
    /// not sleeps):
    ///
    /// 1. legitimate service verified and Ready;
    /// 2. a health operation passes its initial state/liveness check
    ///    (signalled by `reached`, not assumed);
    /// 3. the service dies before credential transmission;
    /// 4. a replacement listener takes the released port;
    /// 5. the health operation resumes.
    ///
    /// `bypass_liveness_flag` additionally skips the connection's own flag
    /// fast path on resume, so the request is attempted using nothing but
    /// the connection -- the guarantee must not depend on any flag.
    async fn death_between_check_and_send(bypass_liveness_flag: bool) {
        let child = establish_ready_child().await; // step 1
        let port = child.connection.port;

        let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<()>();
        let connection_for_health = child.connection.clone();
        let health_task = tokio::spawn(async move {
            check_service_health_inner(
                &connection_for_health,
                PostLivenessHook::pause(reached_tx, resume_rx, bypass_liveness_flag),
            )
            .await
        });
        // Step 2: block until the health operation has *actually* passed
        // its liveness check and is parked before sending.
        reached_rx
            .await
            .expect("health operation must reach the post-check point");

        kill_process_externally(child.pid); // step 3
        wait_for_failed(&child.handle).await;
        let replacement = ReplacementListener::bind(port).await; // step 4

        let _ = resume_tx.send(()); // step 5
        let result = health_task.await.expect("health task should not panic");

        assert!(
            result.is_err(),
            "a health operation resumed after the service died must not succeed (got {result:?})"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        replacement.assert_untouched(&child.connection.token);
        assert_access_revoked(&child).await;
        child.watcher.await.expect("watcher task should not panic");
    }

    #[tokio::test]
    async fn child_death_between_liveness_check_and_send_never_leaks_credential_to_replacement_listener(
    ) {
        death_between_check_and_send(false).await;
    }

    #[tokio::test]
    async fn child_death_between_liveness_check_and_send_leaks_nothing_even_with_the_liveness_flag_bypassed(
    ) {
        death_between_check_and_send(true).await;
    }

    #[tokio::test]
    async fn lost_authenticated_connection_with_a_live_child_revokes_ready_and_never_redials() {
        // Loss of the authenticated connection must revoke access on its
        // own, independent of the child process exiting. Provoke a real
        // loss without killing anything: ask the (live) child to end the
        // keep-alive connection with `Connection: close`.
        let child = establish_ready_child().await;
        let port = child.connection.port;

        // No credential is attached: this request exists only to make the
        // server end the connection.
        let close_request = Request::builder()
            .method("GET")
            .uri("/health")
            .header("Host", format!("127.0.0.1:{port}"))
            .header("Connection", "close")
            .body(Full::new(Bytes::new()))
            .unwrap();
        child
            .connection
            .pinned
            .send(close_request, MAX_HTTP_RESPONSE_BYTES, HTTP_REQUEST_TIMEOUT)
            .await
            .expect("live child answers");

        wait_for_failed(&child.handle).await;
        assert_eq!(
            child.handle.snapshot().await,
            SupervisorState::Failed {
                reason: FailureReason::AuthenticatedConnectionLost.as_str()
            },
            "a lost connection must be reported as such, not as child exit"
        );

        // Ready is revoked and health fails closed. The lifecycle task
        // then drops the child (kill_on_drop), releasing the port; a
        // replacement claiming it must never be contacted.
        let replacement = ReplacementListener::bind(port).await;
        let result = check_service_health(&child.connection).await;
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(300)).await;
        replacement.assert_untouched(&child.connection.token);
        assert_access_revoked(&child).await;
        child.watcher.await.expect("watcher task should not panic");
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
}
