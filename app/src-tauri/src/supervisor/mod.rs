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
//!              -> InstallingCredential -> Ready
//!     (any state) -> Failed(reason)   [terminal, fail-closed]
//!     Ready -> Stopped                [graceful or forced shutdown]
//! ```
//!
//! Every transition to `Failed` means: no endpoint or token is, or ever
//! was, exposed to the frontend ([`SupervisorHandle::ready_connection`]
//! only ever returns `Some` while the state is exactly `Ready`), and the
//! child is killed if it's still running. There is no fallback to a
//! different endpoint, and no retry, at any stage -- a fresh attempt
//! means a fresh [`SupervisorHandle`], a fresh child, a fresh startup
//! secret, nonce, and session token.

mod b64url;
mod challenge;
#[cfg(windows)]
mod job_object;
mod process;
mod protocol;
mod state;

pub use state::{ReadyConnection, SupervisorHandle, SupervisorState};

use rand::RngCore;
use state::OwnedProcess;
use std::path::Path;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};
use tokio::process::Child;
use tokio::time::timeout;

/// Overall bound on the entire startup sequence (spawn through `Ready`).
/// Generous for a cold Python interpreter + FastAPI import on a slow
/// machine, but still bounded -- "startup timeout" is a required
/// fail-closed condition, not an unbounded wait.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Bound on waiting for the child to exit after a graceful shutdown
/// request before force-killing it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

const STARTUP_SECRET_LEN: usize = 32;
const NONCE_LEN: usize = 16;
const SESSION_TOKEN_LEN: usize = 32;

fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// Runs the complete startup sequence, updating `handle` as it
/// progresses -- `handle.subscribe()`/`snapshot()` are how a caller (e.g.
/// `lib.rs`'s Tauri wiring) observes progress; this function has no
/// return value because there is nothing to return that `handle` doesn't
/// already expose. Never leaves `handle` in anything but `Ready` (success)
/// or `Failed` (every other outcome) when it returns.
///
/// Resolves the executable via `process::resolve(app)` (the only step
/// that needs a real `AppHandle`) and delegates everything else to
/// [`run_startup_with_executable`], which has no Tauri dependency at all
/// and is what the test suite in this module exercises directly, against
/// a real spawned child, without needing a mock `AppHandle`.
pub async fn start(app: AppHandle, handle: SupervisorHandle) {
    handle.set_state(SupervisorState::Starting).await;
    let resolved = process::resolve(&app).map_err(|e| e.to_string());
    let outcome = match resolved {
        Ok((executable, args)) => {
            timeout(
                STARTUP_TIMEOUT,
                run_startup_with_executable(&handle, &executable, &args),
            )
            .await
        }
        Err(reason) => Ok(Err(reason)),
    };
    match outcome {
        Ok(Ok(())) => {} // `run_startup_with_executable` already set `Ready`.
        Ok(Err(reason)) => handle.set_state(SupervisorState::Failed { reason }).await,
        Err(_) => {
            handle
                .set_state(SupervisorState::Failed {
                    reason: "startup_timeout".to_string(),
                })
                .await
        }
    }
}

/// The Tauri-independent core of the startup sequence: spawn, private-
/// channel handshake, D-025 HTTP challenge, credential installation. Only
/// touches `handle` (pure in-memory state) -- no `AppHandle`, no Tauri
/// event emission, no capability/ACL surface. The frontend observes
/// progress by polling `handle.snapshot()` (via the `get_service_status`
/// command in `lib.rs`), not through any callback this function makes.
async fn run_startup_with_executable(
    handle: &SupervisorHandle,
    executable: &Path,
    args: &[String],
) -> Result<(), String> {
    let mut child = process::spawn(executable, args).map_err(|e| e.to_string())?;

    // Must be kept alive for exactly as long as the child is meant to
    // keep running -- see the `job` field doc on `state::OwnedProcess`
    // for why this must not be a short-lived local variable that drops
    // (and kills the child) as soon as this function returns.
    #[cfg(windows)]
    let job = {
        let job = job_object::JobObject::create()
            .map_err(|e| format!("job_object_create_failed: {e}"))?;
        job.assign(&child)
            .map_err(|e| format!("job_object_assign_failed: {e}"))?;
        job
    };

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "child_stdin_unavailable".to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "child_stdout_unavailable".to_string())?;
    drain_stderr(child.stderr.take());

    let secret = random_bytes(STARTUP_SECRET_LEN);
    protocol::write_startup_secret(&mut stdin, &secret)
        .await
        .map_err(|e| e.to_string())?;

    handle.set_state(SupervisorState::AwaitingEndpoint).await;
    let endpoint = race_read(
        &mut child,
        protocol::read_endpoint_ready(&mut stdout),
        "endpoint_ready",
    )
    .await?;
    if endpoint.host != "127.0.0.1" {
        // Mirrors the S1-03 bind contract this process itself never
        // weakens: only ever trust a reported endpoint on the exact
        // documented loopback address.
        return Err(format!("endpoint_not_loopback: {}", endpoint.host));
    }

    handle.set_state(SupervisorState::VerifyingIdentity).await;
    // `pool_max_idle_per_host(0)`: this client is used at most a few times
    // total per app session (the challenge request, then occasional
    // `check_service_health` calls) -- no throughput benefit to pooling,
    // and not reusing connections removes one more variable if a
    // connection-level issue is ever suspected again.
    let http_client = reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|e| e.to_string())?;
    verify_identity(&http_client, &secret, &endpoint.host, endpoint.port).await?;

    handle
        .set_state(SupervisorState::InstallingCredential)
        .await;
    let token = b64url::encode(&random_bytes(SESSION_TOKEN_LEN));
    protocol::write_install_session_token(&mut stdin, &token)
        .await
        .map_err(|e| e.to_string())?;
    race_read(&mut child, protocol::read_ready(&mut stdout), "ready").await?;

    handle
        .set_process(OwnedProcess {
            child,
            stdin,
            stdout,
            #[cfg(windows)]
            job,
        })
        .await;
    handle
        .set_ready(ReadyConnection {
            host: endpoint.host,
            port: endpoint.port,
            token,
            http_client,
        })
        .await;

    Ok(())
}

/// Races reading the next expected protocol frame against the child
/// exiting -- so a child that dies mid-handshake fails fast with a clear
/// reason instead of hanging until the overall `STARTUP_TIMEOUT`.
async fn race_read<T, F>(child: &mut Child, read_future: F, what: &str) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, protocol::ProtocolError>>,
{
    tokio::select! {
        result = read_future => result.map_err(|e| format!("{what}_read_failed: {e}")),
        status = child.wait() => {
            let status = status.map_err(|e| e.to_string())?;
            Err(format!("child_exited_before_{what}: {status}"))
        }
    }
}

async fn verify_identity(
    client: &reqwest::Client,
    secret: &[u8],
    host: &str,
    port: u16,
) -> Result<(), String> {
    let canonical = challenge::canonical_endpoint(host, port);
    let nonce = random_bytes(NONCE_LEN);
    let url = format!("http://{host}:{port}/__startup/challenge");

    let response = client
        .post(&url)
        .json(&serde_json::json!({ "nonce": b64url::encode(&nonce) }))
        .send()
        .await
        .map_err(|e| format!("challenge_request_failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "challenge_rejected_status_{}",
            response.status().as_u16()
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("challenge_response_not_json: {e}"))?;
    let response_b64 = body
        .get("response")
        .and_then(|v| v.as_str())
        .ok_or("challenge_response_missing_field")?;
    let response_bytes =
        b64url::decode(response_b64).map_err(|_| "challenge_response_not_base64url".to_string())?;

    if !challenge::verify(secret, &canonical, &nonce, &response_bytes) {
        return Err("challenge_response_incorrect".to_string());
    }
    Ok(())
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

/// Requests graceful shutdown through the private channel, stops
/// accepting frontend access immediately (before the child has actually
/// exited -- see [`SupervisorHandle::take_process_for_shutdown`]), waits
/// a bounded grace period, then forcefully terminates the exact child if
/// it hasn't exited on its own, and reaps it either way. A no-op if the
/// supervisor never reached `Ready` (or already shut down) -- there is no
/// process to shut down.
pub async fn shutdown(handle: &SupervisorHandle) {
    let Some(process) = handle.take_process_for_shutdown().await else {
        return;
    };
    let mut child = process.child;
    let mut stdin = process.stdin;
    let mut stdout = process.stdout;
    // Field-by-field moves (not `OwnedProcess { .. }` destructuring,
    // which would drop an ignored field -- including this one --
    // immediately at the match site): `_job` must stay alive, bound in
    // this scope, until this function's normal return, i.e. after the
    // child has exited/been killed below. Dropping it early closes the
    // job object's handle, which -- because of
    // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` -- immediately kills the
    // child, exactly the bug this same mistake caused once already (see
    // the `job` field doc on `state::OwnedProcess`).
    #[cfg(windows)]
    let _job = process.job;

    let _ = write_shutdown_best_effort(&mut stdin).await;
    // Best-effort confirmation only -- a missing/slow acknowledgement
    // must never block the real shutdown signal below, which is the
    // child actually exiting.
    let _ = timeout(
        Duration::from_secs(2),
        protocol::read_shutdown_ack(&mut stdout),
    )
    .await;

    if timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

async fn write_shutdown_best_effort<W: AsyncWrite + Unpin>(
    stdin: &mut W,
) -> Result<(), protocol::ProtocolError> {
    protocol::write_shutdown(stdin).await
}

#[cfg(test)]
mod tests {
    //! Two kinds of tests here:
    //!
    //! - Real-process integration tests (`full_handshake_...`,
    //!   `wrong_secret_...`) spawn the *actual* `service/.venv` Python
    //!   interpreter running `evidencegraph_service --supervised` (via
    //!   `process::resolve_dev`, the same dev-only resolver `start` uses)
    //!   and drive the real private channel + real D-025 HTTP challenge
    //!   end-to-end. These are this crate's side of the same proof
    //!   `service/tests/test_supervised_process.py` already established
    //!   from the Python side -- together they confirm the documented
    //!   wire protocol and HMAC construction are implemented identically
    //!   by both real implementations, not just individually
    //!   self-consistent. Requires `uv sync` to have been run in
    //!   `service/` first (see `service/README.md`).
    //! - The mandatory fake-endpoint tests (`fake_http_endpoint_...`)
    //!   stand up a real (but fake/scripted) HTTP responder in place of a
    //!   child's reported endpoint and prove `verify_identity` -- the
    //!   exact function the real handshake calls during
    //!   `VerifyingIdentity` -- rejects it, never proceeding to install a
    //!   session token (that step is unreachable without `verify_identity`
    //!   returning `Ok`, enforced by the `?` in `run_startup_with_executable`
    //!   above, not merely by test convention).

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
        assert!(
            matches!(result, Ok(Ok(()))),
            "startup should succeed: {result:?}"
        );
        assert_eq!(handle.snapshot().await, SupervisorState::Ready);

        let connection = handle
            .ready_connection()
            .await
            .expect("connection present when Ready");
        assert_eq!(connection.host, "127.0.0.1");

        let url = format!("http://{}:{}/health", connection.host, connection.port);
        let response = connection
            .http_client
            .get(&url)
            .bearer_auth(&connection.token)
            .send()
            .await
            .expect("authenticated /health request should succeed");
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["status"], "ok");

        // A request with no token must still be rejected -- the token
        // this test holds is not somehow also accepted unauthenticated.
        let unauthed = connection.http_client.get(&url).send().await.unwrap();
        assert_eq!(unauthed.status(), 401);

        shutdown(&handle).await;
        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());

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
        let client = reqwest::Client::new();
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

        let client = reqwest::Client::new();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_malformed_json_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr = fake_http_endpoint("200 OK", "not json at all".to_string()).await;

        let client = reqwest::Client::new();
        let result = verify_identity(&client, &secret, "127.0.0.1", addr.port()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_http_endpoint_with_error_status_is_rejected() {
        let secret = b"a-real-secret-the-fake-endpoint-never-saw".to_vec();
        let addr =
            fake_http_endpoint("404 Not Found", "{\"detail\":\"Not Found\"}".to_string()).await;

        let client = reqwest::Client::new();
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
}
