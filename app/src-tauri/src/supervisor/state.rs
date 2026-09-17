//! S1-04 supervisor state machine. See `supervisor` module docs for the
//! full transition diagram.
//!
//! The one invariant every Tauri command must respect: endpoint/
//! credential data is reachable *only* through [`SupervisorHandle::
//! ready_connection`], which returns `Some` if and only if the state is
//! currently `Ready`. There is no other way to read the token or port out
//! of this type.

use serde::Serialize;
use std::sync::Arc;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SupervisorState {
    Starting,
    AwaitingEndpoint,
    VerifyingIdentity,
    InstallingCredential,
    Ready,
    Failed { reason: String },
    Stopped,
}

/// The data a `Ready` state makes available -- never constructed or
/// exposed except by `run_startup` succeeding.
#[derive(Clone)]
pub struct ReadyConnection {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub http_client: reqwest::Client,
}

/// The live child process handle and both private-channel stdio halves,
/// retained only so `supervisor::shutdown` can send a graceful shutdown
/// message, best-effort read its acknowledgement, and then wait for/kill
/// the exact child -- never exposed outside this module.
///
/// On Windows, also keeps the [`super::job_object::JobObject`] guard
/// alive for as long as the child itself: that handle must **not** be
/// dropped while the child is still meant to be running -- closing a job
/// object's last handle triggers `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
/// which immediately kills everything still assigned to it. Scoping the
/// guard to a function-local variable in `run_startup_with_executable`
/// instead of storing it here was a real bug this project hit: the child
/// was being killed the instant startup finished, right after reaching
/// `Ready`.
pub(super) struct OwnedProcess {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    #[cfg(windows)]
    pub job: super::job_object::JobObject,
}

struct Inner {
    state: SupervisorState,
    connection: Option<ReadyConnection>,
    process: Option<OwnedProcess>,
}

#[derive(Clone)]
pub struct SupervisorHandle {
    inner: Arc<RwLock<Inner>>,
}

impl Default for SupervisorHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SupervisorHandle {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                state: SupervisorState::Starting,
                connection: None,
                process: None,
            })),
        }
    }

    pub async fn set_state(&self, state: SupervisorState) {
        let mut inner = self.inner.write().await;
        inner.state = state;
    }

    pub(super) async fn set_process(&self, process: OwnedProcess) {
        let mut inner = self.inner.write().await;
        inner.process = Some(process);
    }

    /// Transitions to `Ready` and publishes the connection atomically --
    /// no observer can see a state of `Ready` without the connection
    /// already being present, or vice versa.
    pub(super) async fn set_ready(&self, connection: ReadyConnection) {
        let mut inner = self.inner.write().await;
        inner.connection = Some(connection);
        inner.state = SupervisorState::Ready;
    }

    pub async fn snapshot(&self) -> SupervisorState {
        self.inner.read().await.state.clone()
    }

    /// Returns the ready connection *only* if the state is currently
    /// `Ready` -- the one gate every frontend-facing command goes
    /// through. A state transition away from `Ready` (e.g. `Stopped`)
    /// makes this return `None` again even though `connection` itself is
    /// still technically stored, until `take_process`/shutdown clears it.
    pub async fn ready_connection(&self) -> Option<ReadyConnection> {
        let inner = self.inner.read().await;
        match inner.state {
            SupervisorState::Ready => inner.connection.clone(),
            _ => None,
        }
    }

    /// Takes ownership of the child process/stdin for shutdown, and stops
    /// treating the state as `Ready` immediately (before the process has
    /// actually exited) -- "stop accepting frontend access immediately."
    /// A true no-op (state and connection untouched) if the supervisor was
    /// never `Ready` in the first place -- there is nothing to shut down.
    pub(super) async fn take_process_for_shutdown(&self) -> Option<OwnedProcess> {
        let mut inner = self.inner.write().await;
        if matches!(inner.state, SupervisorState::Ready) {
            inner.connection = None;
            inner.state = SupervisorState::Stopped;
        }
        inner.process.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_in_starting_state_with_no_connection() {
        let handle = SupervisorHandle::new();
        assert_eq!(handle.snapshot().await, SupervisorState::Starting);
        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn ready_connection_is_none_until_set_ready_is_called() {
        let handle = SupervisorHandle::new();
        handle
            .set_state(SupervisorState::InstallingCredential)
            .await;
        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn ready_connection_available_only_after_set_ready() {
        let handle = SupervisorHandle::new();
        handle
            .set_ready(ReadyConnection {
                host: "127.0.0.1".into(),
                port: 12345,
                token: "abc".into(),
                http_client: reqwest::Client::new(),
            })
            .await;

        assert_eq!(handle.snapshot().await, SupervisorState::Ready);
        let conn = handle.ready_connection().await.expect("connection present");
        assert_eq!(conn.port, 12345);
    }

    #[tokio::test]
    async fn failed_state_never_exposes_a_connection() {
        let handle = SupervisorHandle::new();
        handle
            .set_ready(ReadyConnection {
                host: "127.0.0.1".into(),
                port: 1,
                token: "abc".into(),
                http_client: reqwest::Client::new(),
            })
            .await;
        handle
            .set_state(SupervisorState::Failed {
                reason: "child_exited".into(),
            })
            .await;

        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn take_process_for_shutdown_clears_connection_and_sets_stopped() {
        let handle = SupervisorHandle::new();
        handle
            .set_ready(ReadyConnection {
                host: "127.0.0.1".into(),
                port: 1,
                token: "abc".into(),
                http_client: reqwest::Client::new(),
            })
            .await;

        let process = handle.take_process_for_shutdown().await;
        assert!(process.is_none()); // none was ever set via `set_process` in this test
        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());
    }
}
