//! S1-04 review round 2, finding 1 (child-death-to-port-reuse credential
//! disclosure): the review correctly rejected the prior mitigation (a
//! `watch`-channel liveness flag checked immediately before and after
//! dialing a *fresh* connection per request) as insufficient -- checking
//! liveness and then opening a brand-new `TcpStream::connect` is still two
//! separate steps, and the freed port can be claimed by a replacement
//! listener in between them, with no way for the caller to tell it apart
//! from the real child.
//!
//! The fix implemented here: dial the child's endpoint **exactly once**,
//! at the moment its identity is cryptographically verified (the D-025
//! challenge response is read over this exact connection), and reuse that
//! *same* TCP connection -- HTTP/1.1 keep-alive, driven by `hyper`'s
//! low-level client connection API -- for every subsequent authenticated
//! request for the rest of the supervised session. Nothing in this module
//! ever calls `TcpStream::connect` a second time for a given
//! [`PinnedConnection`]. If the connection is ever lost, for any reason
//! (the child exits, the socket resets, a protocol error, a clean close),
//! [`PinnedConnection::send`] and [`PinnedConnection::is_alive`] both
//! reflect that immediately and permanently -- there is no reconnect path
//! to accidentally trigger. See `docs/DECISIONS.md` D-026 for the full
//! design rationale, including why `reqwest`'s pooled/auto-redialing
//! client (the prior design) could not give this guarantee no matter how
//! its pool size or liveness checks were tuned.
//!
//! **One deadline per call** (S1-04 review round 3, finding 2): every
//! call to [`PinnedConnection::send`] is bounded by a single deadline,
//! fixed when the call starts, that covers waiting for the connection
//! (calls are serialized -- HTTP/1.1 has no multiplexing), the request
//! itself, and reading the full response body. A call that is still
//! queued when its deadline passes returns `TimedOut` without ever having
//! owned the connection: it wrote nothing, so the connection is left
//! exactly as it was, for whichever caller holds it. A call that fails
//! *after* taking ownership -- deadline, oversized or unreadable body,
//! transport error -- leaves the HTTP/1.1 exchange in an unknown state,
//! so it aborts the connection before releasing it (fail closed): the
//! socket is dropped, [`PinnedConnection::is_alive`] goes false, and
//! every later call fails without I/O. Nothing redials.
//!
//! **Revocation** (S1-04 review round 4): [`PinnedConnection::revoke`] is
//! the one way to end this connection deliberately -- used by the
//! supervisor when it records shutdown or otherwise revokes `Ready`, and
//! by `send` itself when an exchange fails after it started. It closes a
//! gate built into the socket wrapper ([`GatedStream`]): every read and
//! write first takes a read lock on the gate and fails without touching
//! the socket if it is closed, and `revoke` closes it under the write
//! lock. So once `revoke` returns, this process performs no further I/O
//! on the socket -- not for a request queued behind another, not for one
//! that already passed every earlier check, not for a stale clone. This
//! is enforced at the socket, not by a flag a sender checks before
//! writing, so there is no check-then-send window: a request that wrote
//! some bytes before revocation (to the verified peer, the only peer this
//! socket has) fails at its next read or write. `revoke` also wakes the
//! driver task, which drops the socket.
//!
//! Deliberately **not** using `reqwest` here: even with connection pooling
//! disabled, `reqwest::Client::get(...).send()` still dials a fresh
//! connection per call by design -- there is no `reqwest` API for "reuse
//! this exact previously-established connection and never open another
//! one." `hyper::client::conn::http1` is the primitive that actually
//! exposes a single connection as a persistent, explicitly-owned object.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::client::conn::http1::{self, SendRequest};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{watch, Mutex};
use tokio::time::{timeout, timeout_at, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedConnectError {
    Connect,
    ConnectTimedOut,
    Handshake,
}

impl std::fmt::Display for PinnedConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug-build-only diagnostic text (see `supervisor::debug_log`) --
        // deliberately just the fixed variant name, never a wrapped I/O
        // error's own message (S1-04 review finding 4).
        let s = match self {
            Self::Connect => "connect_failed",
            Self::ConnectTimedOut => "connect_timed_out",
            Self::Handshake => "handshake_failed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinnedSendError {
    /// The connection is already known to be closed -- no I/O was
    /// attempted. Distinguished from `Rejected` only so callers/tests can
    /// tell "we never even tried" from "we tried and the OS-level write
    /// failed"; both mean the same thing to a caller: fail closed.
    AlreadyClosed,
    /// `send_request` itself failed (connection reset, broken pipe, the
    /// background driver task already ended, ...).
    Rejected,
    /// The call's single deadline passed -- while queued for the
    /// connection, mid-request, or mid-body.
    TimedOut,
    /// The response body exceeded the caller's byte limit.
    BodyTooLarge,
    /// The response body could not be read to completion.
    BodyReadFailed,
}

impl std::fmt::Display for PinnedSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::AlreadyClosed => "already_closed",
            Self::Rejected => "rejected",
            Self::TimedOut => "timed_out",
            Self::BodyTooLarge => "body_too_large",
            Self::BodyReadFailed => "body_read_failed",
        };
        f.write_str(s)
    }
}

/// The revocation gate shared by a [`PinnedConnection`] and its socket.
/// `closed` is only ever set, never cleared.
#[derive(Default)]
struct Gate {
    closed: RwLock<bool>,
}

impl Gate {
    fn close(&self) {
        // Waits for any read/write currently inside the gate (a single
        // non-blocking socket poll) to finish, so none starts afterwards.
        *self.closed.write().unwrap_or_else(PoisonError::into_inner) = true;
    }

    fn is_closed(&self) -> bool {
        *self.closed.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs one socket poll while holding the gate open, or fails without
    /// touching the socket if it is closed.
    fn with_open<T>(&self, poll: impl FnOnce() -> Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        let closed = self.closed.read().unwrap_or_else(PoisonError::into_inner);
        if *closed {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        poll()
    }
}

/// The TCP stream with every read and write routed through the [`Gate`].
/// Shutting down the write side is left ungated: it sends no request data.
struct GatedStream {
    inner: TcpStream,
    gate: Arc<Gate>,
}

impl AsyncRead for GatedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let inner = &mut this.inner;
        this.gate.with_open(|| Pin::new(inner).poll_read(cx, buf))
    }
}

impl AsyncWrite for GatedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let inner = &mut this.inner;
        this.gate.with_open(|| Pin::new(inner).poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let inner = &mut this.inner;
        this.gate
            .with_open(|| Pin::new(inner).poll_write_vectored(cx, bufs))
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let inner = &mut this.inner;
        this.gate.with_open(|| Pin::new(inner).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A single, explicitly owned HTTP/1.1 connection to the verified child.
/// Cheap to clone -- every clone shares the same underlying connection
/// (requests are serialized through a `Mutex`, which is not a throughput
/// regression versus the prior design: HTTP/1.1 has no request
/// multiplexing on one connection regardless, and this process sends at
/// most a handful of requests total per session).
#[derive(Clone)]
pub struct PinnedConnection {
    sender: Arc<Mutex<SendRequest<Full<Bytes>>>>,
    alive: watch::Receiver<bool>,
    /// Closed by [`revoke`](Self::revoke); checked inside every socket
    /// read/write -- see module docs.
    gate: Arc<Gate>,
    /// Cancelled by `revoke` (after closing the gate) to wake the driver
    /// task, which then drops the socket.
    abort: CancellationToken,
    /// Test-only: fires when a `send` is about to wait for the sender, so
    /// tests can establish "queued behind another request" explicitly.
    #[cfg(test)]
    queued_probe: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

/// A fully read response: status plus the complete, size-bounded body.
#[derive(Debug)]
pub struct PinnedResponse {
    pub status: StatusCode,
    pub body: Vec<u8>,
}

impl PinnedConnection {
    /// Opens the **one** TCP connection this value will ever use, and
    /// performs the HTTP/1.1 handshake over it. Spawns a background task
    /// to drive the connection for its entire life (the standard `hyper`
    /// pattern -- without something polling the returned `Connection`
    /// future, no request made through the paired `SendRequest` can ever
    /// make progress). When that task ends, for *any* reason, `alive` is
    /// flipped to `false` and stays that way forever; nothing in this
    /// type ever calls `connect` again to replace it.
    pub async fn connect(
        host: &str,
        port: u16,
        connect_timeout: Duration,
    ) -> Result<Self, PinnedConnectError> {
        let stream = timeout(connect_timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| PinnedConnectError::ConnectTimedOut)?
            .map_err(|_| PinnedConnectError::Connect)?;
        let gate = Arc::new(Gate::default());
        let io = TokioIo::new(GatedStream {
            inner: stream,
            gate: gate.clone(),
        });

        let (sender, conn) = http1::Builder::new()
            .handshake::<_, Full<Bytes>>(io)
            .await
            .map_err(|_| PinnedConnectError::Handshake)?;

        let (alive_tx, alive_rx) = watch::channel(true);
        let abort = CancellationToken::new();
        let abort_driver = abort.clone();
        // Drives the connection for as long as it lives. This is the only
        // task anywhere that ever touches this TCP stream's I/O; when it
        // returns (cleanly or via an error -- both are folded together,
        // since a raw `hyper::Error` here could carry a peer-controlled
        // detail string, and finding 4 requires never logging/exposing
        // that), or is aborted (dropping `conn` closes the socket), the
        // connection is permanently gone.
        tokio::spawn(async move {
            tokio::select! {
                biased;
                () = abort_driver.cancelled() => {}
                _ = conn => {}
            }
            let _ = alive_tx.send(false);
        });

        Ok(Self {
            sender: Arc::new(Mutex::new(sender)),
            alive: alive_rx,
            gate,
            abort,
            #[cfg(test)]
            queued_probe: Arc::default(),
        })
    }

    /// Permanently ends this connection for every clone: once this
    /// returns, no further byte is read from or written to the socket by
    /// this process (see module docs). Idempotent.
    pub fn revoke(&self) {
        self.gate.close();
        self.abort.cancel();
    }

    /// Test-only: the returned receiver resolves when the next `send` on
    /// any clone is about to wait for the sender.
    #[cfg(test)]
    pub(super) fn notify_when_next_send_queues(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.queued_probe.lock().unwrap() = Some(tx);
        rx
    }

    /// A cheap, immediate check -- does not itself perform any I/O. Used
    /// as a fast pre-check before attempting a send, but -- per the
    /// finding this module exists to fix -- never relied upon *alone* to
    /// prove the connection is still good: the real proof is that
    /// `send`'s own OS-level write either succeeds or fails, and it can
    /// never target anything other than this exact already-established
    /// socket.
    pub fn is_alive(&self) -> bool {
        !self.gate.is_closed() && *self.alive.borrow()
    }

    /// Resolves once this connection is observed closed -- lets a caller
    /// race it in a `tokio::select!` (see `supervisor::watch_ready_process`)
    /// to proactively revoke `Ready` the instant the connection drops,
    /// rather than only discovering it reactively at the next request.
    pub async fn closed(&self) {
        let mut alive = self.alive.clone();
        tokio::select! {
            () = self.abort.cancelled() => {}
            () = async {
                while *alive.borrow() {
                    if alive.changed().await.is_err() {
                        return;
                    }
                }
            } => {}
        }
    }

    /// Sends a request over this exact connection and reads the whole
    /// response body (at most `max_body_bytes`), all within one deadline
    /// of `request_timeout` from now -- see module docs. Never dials
    /// anything -- if the connection is already closed, this fails
    /// immediately without attempting I/O; if it closes concurrently with
    /// the attempt, the underlying write fails and that failure is
    /// surfaced here, not silently retried against a new connection.
    pub async fn send(
        &self,
        request: Request<Full<Bytes>>,
        max_body_bytes: usize,
        request_timeout: Duration,
    ) -> Result<PinnedResponse, PinnedSendError> {
        if !self.is_alive() {
            return Err(PinnedSendError::AlreadyClosed);
        }
        self.send_on_connection(request, max_body_bytes, request_timeout, true)
            .await
    }

    /// Test-only: performs the I/O half of [`send`](Self::send) *without*
    /// consulting the liveness flag. The security property of this type is
    /// that a request can only ever be written to the one already-open
    /// socket -- not that a flag was checked first (a flag check cannot
    /// close the check-then-use window). Tests use this to prove the
    /// property holds even when the flag is stale or bypassed entirely.
    #[cfg(test)]
    pub(super) async fn send_ignoring_liveness_flag(
        &self,
        request: Request<Full<Bytes>>,
        max_body_bytes: usize,
        request_timeout: Duration,
    ) -> Result<PinnedResponse, PinnedSendError> {
        self.send_on_connection(request, max_body_bytes, request_timeout, false)
            .await
    }

    async fn send_on_connection(
        &self,
        request: Request<Full<Bytes>>,
        max_body_bytes: usize,
        request_timeout: Duration,
        check_abort: bool,
    ) -> Result<PinnedResponse, PinnedSendError> {
        let deadline = Instant::now() + request_timeout;

        // Queued behind another call: the wait counts against this call's
        // deadline. Timing out here drops only the pending lock
        // acquisition (cancel-safe) -- nothing was written, so the
        // connection is untouched and stays usable by its current owner.
        #[cfg(test)]
        if let Some(probe) = self.queued_probe.lock().unwrap().take() {
            // Sent in the same poll that then parks on the lock, so a
            // current-thread test runtime cannot observe it before this
            // call is actually waiting.
            let _ = probe.send(());
        }
        let Ok(mut sender) = timeout_at(deadline, self.sender.lock()).await else {
            return Err(PinnedSendError::TimedOut);
        };

        // Fast path: the connection was revoked while this call was
        // queued (by a failed previous owner, or by the supervisor). Not
        // what prevents I/O after revocation -- the gate in the socket
        // does that, even when this is skipped -- just a clean "never
        // tried" result.
        if check_abort && self.gate.is_closed() {
            return Err(PinnedSendError::AlreadyClosed);
        }

        let result = timeout_at(deadline, async {
            sender
                .ready()
                .await
                .map_err(|_| PinnedSendError::Rejected)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| PinnedSendError::Rejected)?;
            let status = response.status();
            let body = read_bounded_body(response.into_body(), max_body_bytes).await?;
            Ok(PinnedResponse { status, body })
        })
        .await
        .unwrap_or(Err(PinnedSendError::TimedOut));

        if result.is_err() {
            // Fail closed while still holding the sender, so no queued
            // call can start an exchange on a connection whose request/
            // response state is now unknown.
            self.revoke();
        }
        drop(sender);
        result
    }
}

/// Reads a response body incrementally, enforcing `max_bytes` against
/// bytes *actually received* -- never `Content-Length`, mirroring the
/// same bounded-read discipline the prior `reqwest`-based implementation
/// used (S1-04 review finding 3). Has no deadline of its own: it only
/// ever runs inside [`PinnedConnection::send`]'s single per-call
/// deadline, so a responder that sends headers and then stalls or
/// trickles bytes cannot hang the caller.
async fn read_bounded_body(
    mut body: Incoming,
    max_bytes: usize,
) -> Result<Vec<u8>, PinnedSendError> {
    let mut buf = Vec::new();
    loop {
        let Some(frame) = body.frame().await else {
            return Ok(buf);
        };
        let frame = frame.map_err(|_| PinnedSendError::BodyReadFailed)?;
        if let Some(data) = frame.data_ref() {
            buf.extend_from_slice(data);
            if buf.len() > max_bytes {
                return Err(PinnedSendError::BodyTooLarge);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const BODY: usize = 4096;

    async fn respond_once(listener: TcpListener, response: &'static str) {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(response.as_bytes()).await;
        }
    }

    fn get_request(host: &str, port: u16, path: &str) -> Request<Full<Bytes>> {
        Request::builder()
            .method("GET")
            .uri(path)
            .header("Host", format!("{host}:{port}"))
            .body(Full::new(Bytes::new()))
            .unwrap()
    }

    #[tokio::test]
    async fn connect_and_send_a_request_over_the_single_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(respond_once(
            listener,
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        ));

        let conn = PinnedConnection::connect("127.0.0.1", addr.port(), Duration::from_secs(3))
            .await
            .unwrap();
        assert!(conn.is_alive());

        let response = conn
            .send(
                get_request("127.0.0.1", addr.port(), "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ok");
    }

    #[tokio::test]
    async fn connect_fails_against_a_closed_port_without_retrying() {
        // Bind then immediately drop -- the port is very likely free
        // again, but nothing is listening.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let result = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(2)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_fails_after_the_server_closes_and_never_redials_the_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                // Accept the connection, then drop it without responding
                // -- simulates the child dying mid-request.
                let _ = close_rx.await;
                drop(socket);
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", addr.port(), Duration::from_secs(3))
            .await
            .unwrap();

        // Close the peer side of the connection.
        let _ = close_tx.send(());
        // Give the background driver task a moment to observe the close.
        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("connection should be observed closed");
        assert!(!conn.is_alive());

        // Bind a *different* listener on the *same* port -- this is only
        // possible because nothing above (including this line) ever
        // dialed it again; the original connection's own socket held no
        // claim on the port once its peer closed it.
        let replacement = TcpListener::bind(("127.0.0.1", addr.port())).await;
        let accepted_a_connection = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(replacement) = replacement {
            let flag = accepted_a_connection.clone();
            tokio::spawn(async move {
                if tokio::time::timeout(Duration::from_millis(300), replacement.accept())
                    .await
                    .is_ok()
                {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }

        let result = conn
            .send(
                get_request("127.0.0.1", addr.port(), "/health"),
                BODY,
                Duration::from_secs(2),
            )
            .await;
        assert!(result.is_err(), "send over a closed connection must fail");

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            !accepted_a_connection.load(std::sync::atomic::Ordering::SeqCst),
            "PinnedConnection must never dial a new connection to the same port"
        );
    }

    #[tokio::test]
    async fn sequential_requests_over_one_connection_never_open_a_second_one() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accept_count_clone = accept_count.clone();
        tokio::spawn(async move {
            // Accept exactly one connection, then serve two HTTP/1.1
            // responses over it sequentially (keep-alive, no
            // `Connection: close`). A second `accept()` deliberately never
            // runs -- if `PinnedConnection` tried to open a second
            // connection, this test would hang/fail on the second send.
            if let Ok((mut socket, _)) = listener.accept().await {
                accept_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                for _ in 0..2 {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await;
                }
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", addr.port(), Duration::from_secs(3))
            .await
            .unwrap();

        for _ in 0..2 {
            let response = conn
                .send(
                    get_request("127.0.0.1", addr.port(), "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
                .unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.body, b"ok");
        }

        assert_eq!(accept_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Accepts connections in a loop for the life of the test, counting
    /// them and recording every byte received -- so any redial by the
    /// client under test is observable, not just the first connection.
    struct CountingListener {
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        received: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl CountingListener {
        fn spawn(listener: TcpListener) -> Self {
            let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let (a, r) = (accepted.clone(), received.clone());
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let r = r.clone();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        while let Ok(n) = socket.read(&mut buf).await {
                            if n == 0 {
                                break;
                            }
                            r.lock().unwrap().extend_from_slice(&buf[..n]);
                        }
                    });
                }
            });
            Self { accepted, received }
        }

        fn accepted(&self) -> usize {
            self.accepted.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn received_len(&self) -> usize {
            self.received.lock().unwrap().len()
        }
    }

    fn authorized_request(host: &str, port: u16) -> Request<Full<Bytes>> {
        Request::builder()
            .method("GET")
            .uri("/health")
            .header("Host", format!("{host}:{port}"))
            .header("Authorization", "Bearer super-secret-session-token")
            .body(Full::new(Bytes::new()))
            .unwrap()
    }

    #[tokio::test]
    async fn write_on_a_dead_connection_never_reaches_a_replacement_even_with_the_flag_bypassed() {
        // The check-then-use window, made explicit: the peer dies, a
        // replacement takes the same port, and a request is sent WITHOUT
        // consulting the liveness flag at all (as if the flag were stale).
        // The request may only ever target the old socket.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                let _ = close_rx.await;
                drop(socket);
            }
            // `listener` is dropped here too, releasing the port.
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        assert!(conn.is_alive());
        let _ = close_tx.send(());
        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("connection should be observed closed");

        // Replacement takes the released port. Retry the bind briefly: the
        // original listener is dropped on the task above.
        let mut replacement = None;
        for _ in 0..100 {
            if let Ok(l) = TcpListener::bind(("127.0.0.1", port)).await {
                replacement = Some(l);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let replacement =
            CountingListener::spawn(replacement.expect("replacement must bind the freed port"));

        let result = conn
            .send_ignoring_liveness_flag(
                authorized_request("127.0.0.1", port),
                BODY,
                Duration::from_secs(2),
            )
            .await;
        assert!(
            result.is_err(),
            "a request over a dead connection must fail"
        );

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(replacement.accepted(), 0, "replacement saw a connection");
        assert_eq!(replacement.received_len(), 0, "replacement received bytes");
    }

    #[tokio::test]
    async fn no_transparent_retry_when_the_peer_drops_the_connection_mid_request() {
        // hyper-util's pooled `Client` will retry an idempotent request on
        // a fresh connection in some failure modes. The low-level
        // connection API must not. The listener keeps accepting, so a
        // retry on a new connection would be counted.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepted_clone = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                accepted_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // Read the request, then drop without any response.
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                drop(socket);
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let result = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await;
        assert!(result.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            accepted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a failed request must not be retried on a new connection"
        );
        assert!(!conn.is_alive());
    }

    #[tokio::test]
    async fn no_reconnect_after_the_server_answers_connection_close() {
        // A well-behaved server may end keep-alive with `Connection:
        // close`. The next request must fail, not silently redial.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepted_clone = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                accepted_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok",
                    )
                    .await;
                drop(socket);
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let first = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await
            .unwrap();
        assert_eq!(first.body, b"ok");

        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("connection should close after `Connection: close`");
        let second = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await;
        assert!(second.is_err());
        let second_ignoring_flag = conn
            .send_ignoring_liveness_flag(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await;
        assert!(second_ignoring_flag.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stalled_response_body_is_bounded_by_the_read_timeout() {
        // Headers arrive, then the body stalls forever. The call's single
        // deadline covers the body too, so a hostile or hung responder
        // cannot hang the caller after sending headers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK
Content-Length: 100

ab",
                    )
                    .await;
                std::future::pending::<()>().await;
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let result = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_millis(300),
            )
            .await;
        assert_eq!(result.err(), Some(PinnedSendError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
        // Fail closed: a mid-body timeout leaves the exchange in an
        // unknown state, so the connection is torn down.
        assert!(!conn.is_alive());
    }

    #[tokio::test]
    async fn a_trickled_response_body_is_bounded_by_the_overall_read_timeout() {
        // One byte at a time, each well inside any per-read idle window:
        // the deadline must cover the whole body, not each frame.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 200 OK
Content-Length: 1000

",
                    )
                    .await;
                loop {
                    if socket.write_all(b"x").await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let result = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_millis(400),
            )
            .await;
        assert_eq!(result.err(), Some(PinnedSendError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_request_that_times_out_leaves_no_usable_connection_and_never_redials() {
        // The server accepts and reads the request but never answers. The
        // send times out. The connection must not remain a usable session
        // and must never lead to a second dial.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepted_clone = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                accepted_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    std::future::pending::<()>().await;
                });
            }
        });

        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let first = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_millis(300),
            )
            .await;
        assert_eq!(first.err(), Some(PinnedSendError::TimedOut));

        // hyper closes a connection whose in-flight request was cancelled
        // (observed, not assumed): the session ends rather than lingering
        // in an indeterminate request/response state.
        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("a timed-out request must end the connection");
        assert!(!conn.is_alive());

        // A later request must fail (not hang, not succeed elsewhere).
        let second = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                BODY,
                Duration::from_millis(500),
            )
            .await;
        assert!(second.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_oversized_body_fails_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(respond_once(
            listener,
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789",
        ));
        let conn = PinnedConnection::connect("127.0.0.1", port, Duration::from_secs(3))
            .await
            .unwrap();
        let result = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                4,
                Duration::from_secs(3),
            )
            .await;
        assert_eq!(result.err(), Some(PinnedSendError::BodyTooLarge));
        assert!(!conn.is_alive());
    }

    // --- S1-04 review round 3, finding 2: one deadline per call ----------

    /// Serves requests on the one accepted connection, sequentially, each
    /// answered `delay` after it's fully received. Reports each request's
    /// arrival on `arrivals`, and counts accepted connections.
    struct SlowServer {
        port: u16,
        accepted: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        arrivals: tokio::sync::mpsc::UnboundedReceiver<()>,
    }

    impl SlowServer {
        async fn start(delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let accepted_clone = accepted.clone();
            let (arrival_tx, arrivals) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    accepted_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let arrival_tx = arrival_tx.clone();
                    tokio::spawn(async move {
                        let mut pending = Vec::new();
                        let mut buf = vec![0u8; 4096];
                        loop {
                            // Each test request is a body-less GET, so one
                            // request ends at each blank line.
                            while let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n")
                            {
                                pending.drain(..end + 4);
                                let _ = arrival_tx.send(());
                                tokio::time::sleep(delay).await;
                                if socket
                                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            match socket.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => pending.extend_from_slice(&buf[..n]),
                            }
                        }
                    });
                }
            });
            Self {
                port,
                accepted,
                arrivals,
            }
        }

        fn accepted(&self) -> usize {
            self.accepted.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Generous allowance for scheduler jitter on a loaded CI machine --
    /// still far below the extra time a queued call would take if its
    /// deadline only started once it owned the connection.
    const SCHEDULING_TOLERANCE: Duration = Duration::from_millis(250);

    #[tokio::test]
    async fn queued_calls_respect_their_overall_deadline() {
        // Each request takes 400ms to answer. Three concurrent calls, each
        // with a 600ms budget. The first owns the connection and finishes
        // at ~400ms. Under the old behaviour (timeout started only after
        // acquiring the lock) the second would finish at ~800ms and the
        // third at ~1200ms -- both well past their 600ms budget.
        let mut server = SlowServer::start(Duration::from_millis(400)).await;
        let conn = PinnedConnection::connect("127.0.0.1", server.port, Duration::from_secs(3))
            .await
            .unwrap();
        let budget = Duration::from_millis(600);

        let first = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                let started = std::time::Instant::now();
                let r = conn
                    .send(get_request("127.0.0.1", port, "/health"), BODY, budget)
                    .await;
                (r, started.elapsed())
            })
        };
        // The first call has written its request (so owns the sender)
        // before the others are queued behind it.
        server.arrivals.recv().await.unwrap();

        let queued: Vec<_> = (0..2)
            .map(|_| {
                let conn = conn.clone();
                let port = server.port;
                tokio::spawn(async move {
                    let started = std::time::Instant::now();
                    let r = conn
                        .send(get_request("127.0.0.1", port, "/health"), BODY, budget)
                        .await;
                    (r, started.elapsed())
                })
            })
            .collect();

        let (first_result, first_elapsed) = first.await.unwrap();
        assert_eq!(first_result.unwrap().body, b"ok");
        assert!(first_elapsed < budget + SCHEDULING_TOLERANCE);

        for task in queued {
            let (result, elapsed) = task.await.unwrap();
            assert!(
                result.is_err(),
                "a queued call cannot finish a 400ms request inside the time left of its 600ms budget"
            );
            assert!(
                elapsed < budget + SCHEDULING_TOLERANCE,
                "queued call took {elapsed:?}, budget {budget:?}"
            );
        }

        // The connection is never left locked: a later call returns
        // promptly (it may fail closed -- one queued call timed out after
        // owning the connection -- but must not hang).
        let started = std::time::Instant::now();
        let _ = conn
            .send(
                get_request("127.0.0.1", server.port, "/health"),
                BODY,
                Duration::from_secs(2),
            )
            .await;
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert_eq!(server.accepted(), 1, "no redial");
    }

    #[tokio::test]
    async fn a_call_that_times_out_while_queued_leaves_the_connection_usable() {
        // The queued call's deadline expires while another call owns the
        // connection. It wrote nothing, so it must not abort or otherwise
        // disturb the connection: the owner completes normally and later
        // calls still work over the same connection.
        let mut server = SlowServer::start(Duration::from_millis(500)).await;
        let conn = PinnedConnection::connect("127.0.0.1", server.port, Duration::from_secs(3))
            .await
            .unwrap();

        let owner = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
            })
        };
        server.arrivals.recv().await.unwrap();

        let started = std::time::Instant::now();
        let queued = conn
            .send(
                get_request("127.0.0.1", server.port, "/health"),
                BODY,
                Duration::from_millis(150),
            )
            .await;
        let elapsed = started.elapsed();
        assert_eq!(queued.err(), Some(PinnedSendError::TimedOut));
        assert!(elapsed < Duration::from_millis(150) + SCHEDULING_TOLERANCE);
        assert!(
            conn.is_alive(),
            "a call that never owned the connection must not tear it down"
        );

        assert_eq!(owner.await.unwrap().unwrap().body, b"ok");
        assert!(conn.is_alive());

        let after = conn
            .send(
                get_request("127.0.0.1", server.port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await
            .expect("the connection is still usable after a queued call timed out");
        assert_eq!(after.body, b"ok");
        assert_eq!(server.accepted(), 1, "no redial");
    }

    #[tokio::test]
    async fn a_cancelled_queued_call_does_not_leave_the_connection_locked() {
        // Dropping a call (e.g. the frontend command's task is cancelled)
        // while it's queued for the connection must release its place.
        let mut server = SlowServer::start(Duration::from_millis(200)).await;
        let conn = PinnedConnection::connect("127.0.0.1", server.port, Duration::from_secs(3))
            .await
            .unwrap();

        let owner = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
            })
        };
        server.arrivals.recv().await.unwrap();

        let queued = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        queued.abort();
        let _ = queued.await;

        assert_eq!(owner.await.unwrap().unwrap().body, b"ok");
        let after = conn
            .send(
                get_request("127.0.0.1", server.port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await
            .expect("connection must not be left locked by a cancelled call");
        assert_eq!(after.body, b"ok");
        assert_eq!(server.accepted(), 1, "no redial");
    }

    #[tokio::test]
    async fn an_in_flight_timeout_fails_queued_calls_closed_without_io() {
        // The owner times out mid-request and aborts the connection while
        // still holding it. A call queued behind it must then fail without
        // writing anything -- the server sees exactly one request.
        let mut server = SlowServer::start(Duration::from_secs(5)).await;
        let conn = PinnedConnection::connect("127.0.0.1", server.port, Duration::from_secs(3))
            .await
            .unwrap();

        let owner = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_millis(300),
                )
                .await
            })
        };
        server.arrivals.recv().await.unwrap();

        let queued = conn
            .send(
                get_request("127.0.0.1", server.port, "/health"),
                BODY,
                Duration::from_secs(3),
            )
            .await;
        assert_eq!(owner.await.unwrap().err(), Some(PinnedSendError::TimedOut));
        assert_eq!(queued.err(), Some(PinnedSendError::AlreadyClosed));
        assert!(!conn.is_alive());
        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("aborted connection is observed closed");

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            server.arrivals.try_recv().is_err(),
            "the queued request must never reach the wire"
        );
        assert_eq!(server.accepted(), 1, "no redial");
    }

    // --- S1-04 review round 4: revocation reaches the transport ----------

    #[tokio::test]
    async fn revoke_stops_a_queued_call_even_with_every_check_bypassed() {
        // One call in flight (the server holds it), a second queued behind
        // it using the test-only path that skips both the liveness fast
        // path and the post-acquisition revoked check. `revoke` alone must
        // stop the queued call from writing anything, and fail the
        // in-flight one.
        let mut server = SlowServer::start(Duration::from_secs(5)).await;
        let conn = PinnedConnection::connect("127.0.0.1", server.port, Duration::from_secs(3))
            .await
            .unwrap();

        let owner = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
            })
        };
        server.arrivals.recv().await.unwrap();

        let queued_signal = conn.notify_when_next_send_queues();
        let queued = {
            let conn = conn.clone();
            let port = server.port;
            tokio::spawn(async move {
                conn.send_ignoring_liveness_flag(
                    get_request("127.0.0.1", port, "/health"),
                    BODY,
                    Duration::from_secs(3),
                )
                .await
            })
        };
        queued_signal.await.unwrap();

        conn.revoke();
        assert!(!conn.is_alive());

        let owner = tokio::time::timeout(Duration::from_secs(2), owner)
            .await
            .expect("the in-flight call fails promptly")
            .unwrap();
        assert!(owner.is_err(), "in-flight call must fail after revoke");
        let queued = tokio::time::timeout(Duration::from_secs(2), queued)
            .await
            .expect("the queued call fails promptly")
            .unwrap();
        assert!(queued.is_err(), "queued call must fail after revoke");

        // Later calls on any clone fail without I/O.
        assert_eq!(
            conn.clone()
                .send(
                    get_request("127.0.0.1", server.port, "/health"),
                    BODY,
                    Duration::from_secs(1),
                )
                .await
                .err(),
            Some(PinnedSendError::AlreadyClosed)
        );
        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("revoked connection is observed closed");

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            server.arrivals.try_recv().is_err(),
            "no request after the first may reach the wire"
        );
        assert_eq!(server.accepted(), 1, "no redial");
    }
}
