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
//! arm_and_spawn`] registers the cancellation token and the lifecycle
//! task together, and [`SupervisorHandle::begin_shutdown`] takes both
//! together with `Option::take()` under the same lock, so a second
//! concurrent or sequential shutdown always observes `None` -- there is
//! no way to cancel twice or await the same lifecycle task twice.
//!
//! **S1-04 review round 3, finding 1** (shutdown vs. readiness
//! publication): `begin_shutdown` records the request in the same locked
//! step that revokes any published connection, and [`SupervisorHandle::
//! set_ready`] checks that record in the same locked step that would
//! publish. The two are serialized by the lock, so once shutdown is
//! recorded no `Ready` state or usable connection can appear.

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
    /// The sending half of the published connection's `process_alive`
    /// signal -- held here so every revocation (child exit, connection
    /// loss, shutdown) flips it in the same locked step that clears
    /// `connection`, including for clones callers already hold.
    connection_alive: Option<watch::Sender<bool>>,
    cancellation_token: Option<CancellationToken>,
    lifecycle_task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Set (permanently) by the first `begin_shutdown`. Once set, nothing
    /// can publish `Ready` or start a lifecycle.
    shutdown_requested: bool,
    #[cfg(test)]
    ever_published_ready: bool,
}

impl Inner {
    /// Clears the published connection and marks every existing clone of
    /// it dead. Caller holds the write lock.
    fn revoke_connection(&mut self) {
        self.connection = None;
        if let Some(alive) = self.connection_alive.take() {
            let _ = alive.send(false);
        }
    }
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
                connection_alive: None,
                cancellation_token: None,
                lifecycle_task: None,
                shutdown_requested: false,
                #[cfg(test)]
                ever_published_ready: false,
            })),
        }
    }

    /// One-time registration of this supervisor's single lifecycle
    /// attempt: stores `token` and the task `spawn` creates, in one locked
    /// step. Returns `false` without calling `spawn` if a lifecycle is
    /// already registered or shutdown has already been recorded -- so a
    /// close request that arrives before startup even began still
    /// prevents a child from ever being launched.
    pub(super) async fn arm_and_spawn(
        &self,
        token: CancellationToken,
        spawn: impl FnOnce() -> tauri::async_runtime::JoinHandle<()>,
    ) -> bool {
        let mut inner = self.inner.write().await;
        if inner.shutdown_requested || inner.cancellation_token.is_some() {
            return false;
        }
        inner.cancellation_token = Some(token);
        // Spawned while the lock is held: the task's first action needs
        // this same lock, so it cannot run ahead of its own registration.
        inner.lifecycle_task = Some(spawn());
        true
    }

    /// Records a shutdown request and revokes any published connection,
    /// in one locked step (see module docs), then hands the cancellation
    /// token and lifecycle task to exactly one caller. Every later (or
    /// concurrent) caller observes `None`. Records the request even when
    /// nothing is armed yet.
    pub(super) async fn begin_shutdown(
        &self,
    ) -> Option<(CancellationToken, tauri::async_runtime::JoinHandle<()>)> {
        let mut inner = self.inner.write().await;
        inner.shutdown_requested = true;
        inner.revoke_connection();
        let token = inner.cancellation_token.take()?;
        let task = inner
            .lifecycle_task
            .take()
            .expect("arm_and_spawn registers the token and task together");
        Some((token, task))
    }

    pub async fn set_state(&self, state: SupervisorState) {
        let mut inner = self.inner.write().await;
        inner.state = state;
    }

    /// Transitions to `Ready` and publishes the connection atomically --
    /// no observer can see a state of `Ready` without the connection
    /// already being present, or vice versa. Refuses (returns `false`,
    /// publishing nothing, and marking `connection` dead) if shutdown has
    /// already been recorded -- decided under the same lock
    /// `begin_shutdown` records under, so there is no window between the
    /// check and the publication.
    pub(super) async fn set_ready(
        &self,
        connection: ReadyConnection,
        alive: watch::Sender<bool>,
    ) -> bool {
        let mut inner = self.inner.write().await;
        if inner.shutdown_requested {
            let _ = alive.send(false);
            return false;
        }
        inner.connection = Some(connection);
        inner.connection_alive = Some(alive);
        inner.state = SupervisorState::Ready;
        #[cfg(test)]
        {
            inner.ever_published_ready = true;
        }
        true
    }

    #[cfg(test)]
    pub(super) async fn ever_published_ready(&self) -> bool {
        self.inner.read().await.ever_published_ready
    }

    /// Atomically clears the connection and transitions to `Failed` --
    /// used by the lifecycle task the instant it detects the child
    /// exited/the channel failed *after* `Ready` was published (S1-04
    /// review finding 1). No caller can observe a state that still says
    /// `Ready` with a connection that no longer corresponds to a live
    /// child.
    pub(super) async fn invalidate_ready(&self, reason: &'static str) {
        let mut inner = self.inner.write().await;
        inner.revoke_connection();
        inner.state = SupervisorState::Failed { reason };
    }

    /// Atomically clears the connection and transitions to `Stopped` --
    /// the graceful-shutdown counterpart of `invalidate_ready`.
    pub(super) async fn mark_stopped(&self) {
        let mut inner = self.inner.write().await;
        inner.revoke_connection();
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
        let (connection, tx) = ready_connection(12345).await;
        assert!(handle.set_ready(connection, tx).await);

        assert_eq!(handle.snapshot().await, SupervisorState::Ready);
        let conn = handle.ready_connection().await.expect("connection present");
        assert_eq!(conn.port, 12345);
    }

    #[tokio::test]
    async fn failed_state_never_exposes_a_connection() {
        let handle = SupervisorHandle::new();
        let (connection, tx) = ready_connection(1).await;
        assert!(handle.set_ready(connection, tx).await);
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
        let (connection, tx) = ready_connection(1).await;
        assert!(handle.set_ready(connection, tx).await);

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
        let (connection, tx) = ready_connection(1).await;
        assert!(handle.set_ready(connection, tx).await);

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

    fn idle_task() -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async {})
    }

    #[tokio::test]
    async fn arm_is_one_time_only() {
        let handle = SupervisorHandle::new();
        let token_a = CancellationToken::new();
        let token_b = CancellationToken::new();

        assert!(handle.arm_and_spawn(token_a.clone(), idle_task).await);
        let mut second_spawned = false;
        assert!(
            !handle
                .arm_and_spawn(token_b.clone(), || {
                    second_spawned = true;
                    idle_task()
                })
                .await
        );
        assert!(!second_spawned, "a refused arm must not spawn anything");

        // The originally-armed token is still the one `shutdown` would
        // cancel.
        let (taken, _task) = handle.begin_shutdown().await.unwrap();
        taken.cancel();
        assert!(token_a.is_cancelled());
        assert!(!token_b.is_cancelled());
    }

    #[tokio::test]
    async fn begin_shutdown_hands_out_the_token_once() {
        let handle = SupervisorHandle::new();
        handle
            .arm_and_spawn(CancellationToken::new(), idle_task)
            .await;

        assert!(handle.begin_shutdown().await.is_some());
        assert!(handle.begin_shutdown().await.is_none());
    }

    // --- S1-04 review round 3, finding 1 ---------------------------------

    #[tokio::test]
    async fn shutdown_recorded_before_start_prevents_arming() {
        let handle = SupervisorHandle::new();
        assert!(handle.begin_shutdown().await.is_none());

        let mut spawned = false;
        assert!(
            !handle
                .arm_and_spawn(CancellationToken::new(), || {
                    spawned = true;
                    idle_task()
                })
                .await
        );
        assert!(!spawned);
    }

    #[tokio::test]
    async fn set_ready_after_shutdown_is_recorded_publishes_nothing() {
        let handle = SupervisorHandle::new();
        handle
            .set_state(SupervisorState::InstallingCredential)
            .await;
        let _ = handle.begin_shutdown().await;

        let (connection, tx) = ready_connection(1).await;
        let probe = connection.clone();
        assert!(!handle.set_ready(connection, tx).await);

        assert!(!handle.ever_published_ready().await);
        assert_ne!(handle.snapshot().await, SupervisorState::Ready);
        assert!(handle.ready_connection().await.is_none());
        assert!(!probe.is_alive(), "a refused connection must be dead");
    }

    #[tokio::test]
    async fn shutdown_recorded_after_set_ready_revokes_the_connection_and_its_clones() {
        let handle = SupervisorHandle::new();
        let (connection, tx) = ready_connection(1).await;
        assert!(handle.set_ready(connection, tx).await);
        let held = handle.ready_connection().await.unwrap();
        assert!(held.is_alive());

        let _ = handle.begin_shutdown().await;

        assert!(handle.ready_connection().await.is_none());
        assert!(!held.is_alive());
    }

    #[tokio::test]
    async fn invalidate_ready_marks_held_clones_dead() {
        let handle = SupervisorHandle::new();
        let (connection, tx) = ready_connection(1).await;
        assert!(handle.set_ready(connection, tx).await);
        let held = handle.ready_connection().await.unwrap();

        handle.invalidate_ready("child_exited_unexpectedly").await;

        assert!(!held.is_alive());
    }
}
