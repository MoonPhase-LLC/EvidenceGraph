//! S1-04 supervisor state machine. See `supervisor` module docs for the
//! full transition diagram.
//!
//! The one invariant every Tauri command must respect: endpoint/
//! credential data is reachable *only* through [`SupervisorHandle::
//! ready_connection`], which returns `Some` if and only if the state is
//! currently `Ready`. There is no other way to read the token or port out
//! of this type.
//!
//! **S1-04 review finding 1** (revoke stale connections after child
//! death -- round 2): [`ReadyConnection`] no longer carries a generic HTTP
//! client. It carries the one [`super::pinned_http::PinnedConnection`]
//! that was dialed exactly once, at the moment the child's identity was
//! cryptographically verified (the D-025 challenge response was read over
//! this exact TCP connection), plus a `watch::Receiver<bool>` the
//! lifecycle task (`supervisor::run_lifecycle`) flips to `false` the
//! instant it detects the child process has exited. [`ReadyConnection::
//! is_alive`] is the logical AND of both signals: the process being known
//! alive, and the pinned connection itself still being open. Neither
//! signal -- nor a request-time check of either -- is what actually
//! prevents credential disclosure to a replacement listener; that
//! guarantee comes from `PinnedConnection` structurally never dialing a
//! second connection to the same address once the first is established
//! (see that module's docs and `docs/DECISIONS.md` D-026).
//!
//! **S1-04 review finding 2** (idempotent shutdown): [`SupervisorHandle::
//! arm`]/[`SupervisorHandle::take_cancellation_token`] and
//! [`SupervisorHandle::set_lifecycle_task`]/[`SupervisorHandle::
//! take_lifecycle_task`] each use `Option::take()` under the same lock,
//! so a second concurrent or sequential call to either "take" method
//! always observes `None` -- there is no way to cancel twice or await the
//! same lifecycle task twice.

use super::pinned_http::PinnedConnection;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SupervisorState {
    Starting,
    AwaitingEndpoint,
    VerifyingIdentity,
    InstallingCredential,
    Ready,
    Failed { reason: &'static str },
    Stopped,
}

/// The data a `Ready` state makes available -- never constructed or
/// exposed except by the lifecycle task's startup sequence succeeding.
#[derive(Clone)]
pub struct ReadyConnection {
    pub host: String,
    pub port: u16,
    pub token: String,
    pub(super) pinned: PinnedConnection,
    process_alive: watch::Receiver<bool>,
}

impl ReadyConnection {
    pub(super) fn new(
        host: String,
        port: u16,
        token: String,
        pinned: PinnedConnection,
        process_alive: watch::Receiver<bool>,
    ) -> Self {
        Self {
            host,
            port,
            token,
            pinned,
            process_alive,
        }
    }

    /// `true` only while *both* signals say so: the lifecycle task has
    /// not observed the child process exit, **and** the one pinned TCP
    /// connection to it is still open. Either going false is permanent
    /// for this `ReadyConnection` -- nothing here ever re-dials or
    /// resets either signal back to `true`. See module docs for why
    /// neither signal alone, nor this combined check alone, is what
    /// actually prevents credential disclosure to a replacement listener
    /// (`PinnedConnection`'s own never-redial guarantee is).
    pub fn is_alive(&self) -> bool {
        *self.process_alive.borrow() && self.pinned.is_alive()
    }
}

struct Inner {
    state: SupervisorState,
    connection: Option<ReadyConnection>,
    cancellation_token: Option<CancellationToken>,
    lifecycle_task: Option<tauri::async_runtime::JoinHandle<()>>,
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
                cancellation_token: None,
                lifecycle_task: None,
            })),
        }
    }

    /// One-time registration of the cancellation token for this
    /// supervisor's single lifecycle attempt. Returns `false` (a no-op)
    /// if a token is already registered -- `supervisor::start` is only
    /// ever expected to call this once per `SupervisorHandle`, but this
    /// keeps a second call safe rather than silently overwriting the
    /// token `shutdown` might already be about to cancel.
    pub(super) async fn arm(&self, token: CancellationToken) -> bool {
        let mut inner = self.inner.write().await;
        if inner.cancellation_token.is_some() {
            return false;
        }
        inner.cancellation_token = Some(token);
        true
    }

    pub(super) async fn set_lifecycle_task(&self, task: tauri::async_runtime::JoinHandle<()>) {
        let mut inner = self.inner.write().await;
        inner.lifecycle_task = Some(task);
    }

    /// Takes the cancellation token exactly once. A second (or
    /// concurrent) caller observes `None` -- this is the core of
    /// `shutdown`'s idempotency: "initiate shutdown once."
    pub(super) async fn take_cancellation_token(&self) -> Option<CancellationToken> {
        let mut inner = self.inner.write().await;
        inner.cancellation_token.take()
    }

    pub(super) async fn take_lifecycle_task(&self) -> Option<tauri::async_runtime::JoinHandle<()>> {
        let mut inner = self.inner.write().await;
        inner.lifecycle_task.take()
    }

    pub async fn set_state(&self, state: SupervisorState) {
        let mut inner = self.inner.write().await;
        inner.state = state;
    }

    /// Transitions to `Ready` and publishes the connection atomically --
    /// no observer can see a state of `Ready` without the connection
    /// already being present, or vice versa.
    pub(super) async fn set_ready(&self, connection: ReadyConnection) {
        let mut inner = self.inner.write().await;
        inner.connection = Some(connection);
        inner.state = SupervisorState::Ready;
    }

    /// Atomically clears the connection and transitions to `Failed` --
    /// used by the lifecycle task the instant it detects the child
    /// exited/the channel failed *after* `Ready` was published (S1-04
    /// review finding 1). No caller can observe a state that still says
    /// `Ready` with a connection that no longer corresponds to a live
    /// child.
    pub(super) async fn invalidate_ready(&self, reason: &'static str) {
        let mut inner = self.inner.write().await;
        inner.connection = None;
        inner.state = SupervisorState::Failed { reason };
    }

    /// Atomically clears the connection and transitions to `Stopped` --
    /// the graceful-shutdown counterpart of `invalidate_ready`.
    pub(super) async fn mark_stopped(&self) {
        let mut inner = self.inner.write().await;
        inner.connection = None;
        inner.state = SupervisorState::Stopped;
    }

    pub async fn snapshot(&self) -> SupervisorState {
        self.inner.read().await.state.clone()
    }

    /// Returns the ready connection *only* if the state is currently
    /// `Ready` -- the one gate every frontend-facing command goes
    /// through. A state transition away from `Ready` (e.g. `Stopped`,
    /// or `Failed` via `invalidate_ready`) makes this return `None`
    /// atomically with that same transition.
    pub async fn ready_connection(&self) -> Option<ReadyConnection> {
        let inner = self.inner.read().await;
        match inner.state {
            SupervisorState::Ready => inner.connection.clone(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Builds a `ReadyConnection` backed by a real (but otherwise unused)
    /// `PinnedConnection` -- these tests exercise `SupervisorHandle`'s
    /// state-management logic (`Option` handling, the combined
    /// `is_alive()` gate), not networking behavior itself, but
    /// `PinnedConnection::connect` needs a real listening socket to
    /// succeed. `port` is the value stored in the returned
    /// `ReadyConnection.port` field for assertions -- independent of the
    /// listener's actual ephemeral port, since nothing here sends traffic
    /// over the pinned connection.
    async fn ready_connection(port: u16) -> (ReadyConnection, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(true);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Held open for the listener's own lifetime so the connection
        // this test constructs doesn't observe an immediate close.
        tokio::spawn(async move {
            let _ = listener.accept().await;
            std::future::pending::<()>().await
        });
        let pinned = PinnedConnection::connect("127.0.0.1", addr.port(), Duration::from_secs(3))
            .await
            .unwrap();
        (
            ReadyConnection::new("127.0.0.1".into(), port, "abc".into(), pinned, rx),
            tx,
        )
    }

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
        let (connection, _tx) = ready_connection(12345).await;
        handle.set_ready(connection).await;

        assert_eq!(handle.snapshot().await, SupervisorState::Ready);
        let conn = handle.ready_connection().await.expect("connection present");
        assert_eq!(conn.port, 12345);
    }

    #[tokio::test]
    async fn failed_state_never_exposes_a_connection() {
        let handle = SupervisorHandle::new();
        let (connection, _tx) = ready_connection(1).await;
        handle.set_ready(connection).await;
        handle
            .set_state(SupervisorState::Failed {
                reason: "child_exited_unexpectedly",
            })
            .await;

        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn invalidate_ready_clears_connection_and_sets_failed_atomically() {
        let handle = SupervisorHandle::new();
        let (connection, _tx) = ready_connection(1).await;
        handle.set_ready(connection).await;

        handle.invalidate_ready("child_exited_unexpectedly").await;

        assert_eq!(
            handle.snapshot().await,
            SupervisorState::Failed {
                reason: "child_exited_unexpectedly"
            }
        );
        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn mark_stopped_clears_connection() {
        let handle = SupervisorHandle::new();
        let (connection, _tx) = ready_connection(1).await;
        handle.set_ready(connection).await;

        handle.mark_stopped().await;

        assert_eq!(handle.snapshot().await, SupervisorState::Stopped);
        assert!(handle.ready_connection().await.is_none());
    }

    #[tokio::test]
    async fn ready_connection_is_alive_reflects_the_watch_channel() {
        let (connection, tx) = ready_connection(1).await;
        assert!(connection.is_alive());

        tx.send(false).unwrap();

        assert!(!connection.is_alive());
    }

    #[tokio::test]
    async fn arm_is_one_time_only() {
        let handle = SupervisorHandle::new();
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();

        assert!(handle.arm(token_a.clone()).await);
        assert!(!handle.arm(token_b.clone()).await);

        // The originally-armed token is still the one `shutdown` would
        // cancel -- confirmed indirectly via `take_cancellation_token`
        // returning something that, when cancelled, matches `token_a`'s
        // own cancellation, not `token_b`'s.
        let taken = handle.take_cancellation_token().await.unwrap();
        taken.cancel();
        assert!(token_a.is_cancelled());
        assert!(!token_b.is_cancelled());
    }

    #[tokio::test]
    async fn take_cancellation_token_is_one_time_only() {
        let handle = SupervisorHandle::new();
        handle.arm(CancellationToken::new()).await;

        assert!(handle.take_cancellation_token().await.is_some());
        assert!(handle.take_cancellation_token().await.is_none());
    }
}
