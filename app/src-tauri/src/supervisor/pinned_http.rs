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
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{watch, Mutex};
use tokio::time::timeout;

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
    TimedOut,
}

impl std::fmt::Display for PinnedSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::AlreadyClosed => "already_closed",
            Self::Rejected => "rejected",
            Self::TimedOut => "timed_out",
        };
        f.write_str(s)
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
        let io = TokioIo::new(stream);

        let (sender, conn) = http1::Builder::new()
            .handshake::<_, Full<Bytes>>(io)
            .await
            .map_err(|_| PinnedConnectError::Handshake)?;

        let (alive_tx, alive_rx) = watch::channel(true);
        // Drives the connection for as long as it lives. This is the only
        // task anywhere that ever touches this TCP stream's I/O; when it
        // returns (cleanly or via an error -- both are folded together,
        // since a raw `hyper::Error` here could carry a peer-controlled
        // detail string, and finding 4 requires never logging/exposing
        // that), the connection is permanently gone.
        tokio::spawn(async move {
            let _ = conn.await;
            let _ = alive_tx.send(false);
        });

        Ok(Self {
            sender: Arc::new(Mutex::new(sender)),
            alive: alive_rx,
        })
    }

    /// A cheap, immediate check -- does not itself perform any I/O. Used
    /// as a fast pre-check before attempting a send, but -- per the
    /// finding this module exists to fix -- never relied upon *alone* to
    /// prove the connection is still good: the real proof is that
    /// `send`'s own OS-level write either succeeds or fails, and it can
    /// never target anything other than this exact already-established
    /// socket.
    pub fn is_alive(&self) -> bool {
        *self.alive.borrow()
    }

    /// Resolves once this connection is observed closed -- lets a caller
    /// race it in a `tokio::select!` (see `supervisor::watch_ready_process`)
    /// to proactively revoke `Ready` the instant the connection drops,
    /// rather than only discovering it reactively at the next request.
    pub async fn closed(&self) {
        let mut alive = self.alive.clone();
        while *alive.borrow() {
            if alive.changed().await.is_err() {
                return;
            }
        }
    }

    /// Sends a request over this exact connection and returns the
    /// response, unread. Never dials anything -- if the connection is
    /// already closed, this fails immediately without attempting I/O; if
    /// it closes concurrently with the attempt, the underlying write
    /// fails and that failure is surfaced here, not silently retried
    /// against a new connection.
    pub async fn send(
        &self,
        request: Request<Full<Bytes>>,
        request_timeout: Duration,
    ) -> Result<Response<Incoming>, PinnedSendError> {
        if !self.is_alive() {
            return Err(PinnedSendError::AlreadyClosed);
        }
        self.send_on_connection(request, request_timeout).await
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
        request_timeout: Duration,
    ) -> Result<Response<Incoming>, PinnedSendError> {
        self.send_on_connection(request, request_timeout).await
    }

    async fn send_on_connection(
        &self,
        request: Request<Full<Bytes>>,
        request_timeout: Duration,
    ) -> Result<Response<Incoming>, PinnedSendError> {
        let mut sender = self.sender.lock().await;
        timeout(request_timeout, async {
            sender
                .ready()
                .await
                .map_err(|_| PinnedSendError::Rejected)?;
            sender
                .send_request(request)
                .await
                .map_err(|_| PinnedSendError::Rejected)
        })
        .await
        .map_err(|_| PinnedSendError::TimedOut)?
    }
}

/// Reads a response body incrementally, enforcing `max_bytes` against
/// bytes *actually received* -- never `Content-Length`, mirroring the
/// same bounded-read discipline the prior `reqwest`-based implementation
/// used (S1-04 review finding 3). `read_timeout` bounds the *whole* body
/// read, so a responder that sends headers and then stalls or trickles
/// bytes cannot hang the caller -- the old client's overall request timeout
/// covered the body, and `send`'s timeout only covers up to the headers.
pub async fn read_bounded_body(
    mut body: Incoming,
    max_bytes: usize,
    read_timeout: Duration,
) -> Result<Vec<u8>, ReadBodyError> {
    timeout(read_timeout, async {
        let mut buf = Vec::new();
        loop {
            let Some(frame) = body.frame().await else {
                return Ok(buf);
            };
            let frame = frame.map_err(|_| ReadBodyError::ReadFailed)?;
            if let Some(data) = frame.data_ref() {
                buf.extend_from_slice(data);
                if buf.len() > max_bytes {
                    return Err(ReadBodyError::TooLarge);
                }
            }
        }
    })
    .await
    .map_err(|_| ReadBodyError::TimedOut)?
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadBodyError {
    ReadFailed,
    TimedOut,
    TooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
                Duration::from_secs(3),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = read_bounded_body(response.into_body(), 4096, Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(body, b"ok");
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
                    Duration::from_secs(3),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let _ = read_bounded_body(response.into_body(), 4096, Duration::from_secs(3))
                .await
                .unwrap();
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
                Duration::from_secs(3),
            )
            .await
            .unwrap();
        let _ = read_bounded_body(first.into_body(), 4096, Duration::from_secs(3)).await;

        tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .expect("connection should close after `Connection: close`");
        let second = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                Duration::from_secs(3),
            )
            .await;
        assert!(second.is_err());
        let second_ignoring_flag = conn
            .send_ignoring_liveness_flag(
                get_request("127.0.0.1", port, "/health"),
                Duration::from_secs(3),
            )
            .await;
        assert!(second_ignoring_flag.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stalled_response_body_is_bounded_by_the_read_timeout() {
        // Headers arrive, then the body stalls forever. `send`'s timeout
        // only covers up to the headers, so without a deadline on the body
        // read a hostile or hung responder could hang the caller.
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
        let response = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                Duration::from_secs(3),
            )
            .await
            .unwrap();

        let started = std::time::Instant::now();
        let result =
            read_bounded_body(response.into_body(), 4096, Duration::from_millis(300)).await;
        assert_eq!(result, Err(ReadBodyError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(2));
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
        let response = conn
            .send(
                get_request("127.0.0.1", port, "/health"),
                Duration::from_secs(3),
            )
            .await
            .unwrap();
        let result =
            read_bounded_body(response.into_body(), 4096, Duration::from_millis(400)).await;
        assert_eq!(result, Err(ReadBodyError::TimedOut));
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
                Duration::from_millis(500),
            )
            .await;
        assert!(second.is_err());

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
